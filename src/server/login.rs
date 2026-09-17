use crate::lark::project_app::{FeishuLoginApp, ProjectFeishuAppStore};
use crate::server::api::RequiredJsonBody;
use crate::server::error::{ApiError, ApiResult};
use crate::server::state::state_from_depot;
use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use chrono::{DateTime, Duration, Utc};
use jsonwebtoken::{Algorithm, DecodingKey, EncodingKey, Header, Validation, decode, encode};
use salvo::http::header::{CACHE_CONTROL, HeaderValue, PRAGMA};
use salvo::oapi::ToSchema;
use salvo::prelude::*;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use sqlx::{PgPool, Row};
use std::collections::HashSet;
use std::env;
use std::sync::Arc;
use url::Url;

const SESSION_LIFETIME_DAYS: i64 = 7;
const STATE_LIFETIME_MINUTES: i64 = 10;
const SESSION_AUDIENCE: &str = "lark-utils-exp";
const STATE_AUDIENCE: &str = "lark-utils-exp-feishu-oauth-state";
const DEFAULT_ISSUER: &str = "lark-utils-exp";

#[derive(Clone)]
pub struct LoginService {
    pool: PgPool,
    project_apps: ProjectFeishuAppStore,
    jwt_secret: Option<Arc<Vec<u8>>>,
    issuer: String,
    redirect_uris: Arc<HashSet<String>>,
    authorization_url: String,
    openapi_base_url: String,
    internal_api_token: Option<String>,
    http: reqwest::Client,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoginClaims {
    pub sub: String,
    pub jti: String,
    pub iss: String,
    pub aud: String,
    pub iat: i64,
    pub exp: i64,
    pub feishu_app_id: Option<i64>,
    pub is_default_app: bool,
}

#[derive(Debug, Serialize, Deserialize)]
struct LoginStateClaims {
    iss: String,
    aud: String,
    iat: i64,
    exp: i64,
    nonce: String,
    feishu_app_id: Option<i64>,
    redirect_uri: String,
}

#[derive(Debug, Clone)]
pub struct AuthenticatedSession {
    pub session_id: String,
    pub union_id: String,
    pub open_id: String,
    pub user_name: String,
    pub avatar_url: Option<String>,
    pub feishu_app_id: Option<i64>,
    pub is_default_app: bool,
    pub expires_at: DateTime<Utc>,
    pub project_permissions: Vec<ProjectPermission>,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct ProjectPermission {
    pub project_id: i64,
    pub can_view: bool,
    pub can_manage: bool,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct LoginAppDto {
    pub feishu_app_id: Option<i64>,
    pub display_name: String,
    pub is_default: bool,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct AuthorizeRequest {
    /// 不传表示使用 DEFAULT_FEISHU_APP_ID 指定的数据库默认飞书应用。
    pub feishu_app_id: Option<i64>,
    /// 必须与 AUTH_REDIRECT_URIS 中配置的完整地址完全一致。
    pub redirect_uri: String,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct AuthorizeResponse {
    pub authorization_url: String,
    pub state: String,
    pub expires_at: DateTime<Utc>,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct CallbackRequest {
    pub code: String,
    pub state: String,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct LoginUserDto {
    pub union_id: String,
    pub open_id: String,
    pub name: String,
    pub avatar_url: Option<String>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct LoginResponse {
    pub access_token: String,
    pub token_type: String,
    pub expires_in: i64,
    pub expires_at: DateTime<Utc>,
    pub is_default_app: bool,
    pub feishu_app_id: Option<i64>,
    pub user: LoginUserDto,
    pub project_permissions: Vec<ProjectPermission>,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct AppTokenLoginRequest {
    /// 默认管理员为非默认飞书应用生成的长期 `sk-...` Token。
    pub token: String,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct AppLoginTokenDto {
    pub configured: bool,
    pub is_active: bool,
    /// 脱敏前缀，仅用于区分 Token，不能用于登录。
    pub token_prefix: Option<String>,
    pub remark: Option<String>,
    pub created_at: Option<DateTime<Utc>>,
    pub updated_at: Option<DateTime<Utc>>,
    pub last_used_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct RotatedAppLoginTokenDto {
    #[serde(flatten)]
    pub status: AppLoginTokenDto,
    /// 只在本次生成/轮换响应中返回一次，请立即安全保存。
    pub token: String,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct MeResponse {
    pub is_default_app: bool,
    pub feishu_app_id: Option<i64>,
    pub expires_at: DateTime<Utc>,
    pub user: LoginUserDto,
    pub project_permissions: Vec<ProjectPermission>,
}

#[derive(Debug, Deserialize)]
struct FeishuTokenResponse {
    #[serde(default)]
    code: Option<i64>,
    #[serde(default)]
    msg: Option<String>,
    #[serde(default)]
    error: Option<String>,
    #[serde(default)]
    error_description: Option<String>,
    access_token: Option<String>,
}

#[derive(Debug, Deserialize)]
struct FeishuUserEnvelope {
    code: i64,
    msg: String,
    data: Option<FeishuUserInfo>,
}

#[derive(Debug, Deserialize)]
struct FeishuUserInfo {
    name: String,
    open_id: String,
    union_id: String,
    avatar_url: Option<String>,
}

impl LoginService {
    pub fn from_env(pool: PgPool, project_apps: ProjectFeishuAppStore) -> anyhow::Result<Self> {
        let jwt_secret = env::var("AUTH_JWT_SECRET")
            .ok()
            .map(|value| value.trim().as_bytes().to_vec())
            .filter(|value| !value.is_empty());
        if jwt_secret.as_ref().is_some_and(|value| value.len() < 32) {
            anyhow::bail!("AUTH_JWT_SECRET 至少需要 32 字节");
        }
        let redirect_uris = parse_redirect_uris(env::var("AUTH_REDIRECT_URIS").ok().as_deref())?;
        let authorization_url = env::var("FEISHU_AUTHORIZATION_URL")
            .unwrap_or_else(|_| "https://accounts.feishu.cn/open-apis/authen/v1/index".to_owned());
        Url::parse(&authorization_url)
            .map_err(|error| anyhow::anyhow!("FEISHU_AUTHORIZATION_URL 无效：{error}"))?;
        let openapi_base_url = env::var("LARK_BASE_URL")
            .unwrap_or_else(|_| "https://open.feishu.cn".to_owned())
            .trim_end_matches('/')
            .to_owned();
        let internal_api_token = env::var("INTERNAL_API_TOKEN")
            .ok()
            .map(|value| value.trim().to_owned())
            .filter(|value| !value.is_empty());
        Ok(Self {
            pool,
            project_apps,
            jwt_secret: jwt_secret.map(Arc::new),
            issuer: env::var("AUTH_JWT_ISSUER").unwrap_or_else(|_| DEFAULT_ISSUER.to_owned()),
            redirect_uris: Arc::new(redirect_uris),
            authorization_url,
            openapi_base_url,
            internal_api_token,
            http: reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(30))
                .build()?,
        })
    }

    pub fn internal_api_token(&self) -> Option<&str> {
        self.internal_api_token.as_deref()
    }

    fn jwt_secret(&self) -> Result<&[u8], ApiError> {
        self.jwt_secret
            .as_deref()
            .map(Vec::as_slice)
            .ok_or_else(|| ApiError::service_unavailable("服务端尚未配置 AUTH_JWT_SECRET"))
    }

    async fn resolve_login_app(
        &self,
        feishu_app_id: Option<i64>,
    ) -> Result<FeishuLoginApp, ApiError> {
        match feishu_app_id {
            None => Ok(self.project_apps.default_login_app()),
            Some(id) if id <= 0 => Err(ApiError::bad_request("feishu_app_id 必须大于 0")),
            Some(id) => self
                .project_apps
                .login_app(id)
                .await
                .map_err(ApiError::internal)?
                .ok_or_else(|| ApiError::not_found("飞书应用不存在")),
        }
    }

    pub async fn list_login_apps(&self) -> Result<Vec<LoginAppDto>, ApiError> {
        let default_app = self.project_apps.default_login_app();
        let default_database_app_id = self.project_apps.default_database_app_id();
        let mut result = vec![LoginAppDto {
            feishu_app_id: None,
            display_name: default_app.display_name,
            is_default: true,
        }];
        let apps = self
            .project_apps
            .list(false)
            .await
            .map_err(ApiError::internal)?;
        result.extend(
            apps.into_iter()
                .filter(|app| Some(app.feishu_app_id) != default_database_app_id)
                .map(|app| LoginAppDto {
                    feishu_app_id: Some(app.feishu_app_id),
                    display_name: app.display_name,
                    is_default: false,
                }),
        );
        Ok(result)
    }

    pub async fn authorize(
        &self,
        request: AuthorizeRequest,
    ) -> Result<AuthorizeResponse, ApiError> {
        let redirect_uri = normalized_redirect_uri(&request.redirect_uri)?;
        if !self.redirect_uris.contains(&redirect_uri) {
            return Err(ApiError::bad_request(
                "redirect_uri 未列入 AUTH_REDIRECT_URIS 白名单",
            ));
        }
        let app = self.resolve_login_app(request.feishu_app_id).await?;
        let now = Utc::now();
        let expires_at = now + Duration::minutes(STATE_LIFETIME_MINUTES);
        let state = encode(
            &Header::new(Algorithm::HS256),
            &LoginStateClaims {
                iss: self.issuer.clone(),
                aud: STATE_AUDIENCE.to_owned(),
                iat: now.timestamp(),
                exp: expires_at.timestamp(),
                nonce: random_token(24)?,
                feishu_app_id: app.feishu_app_id,
                redirect_uri: redirect_uri.clone(),
            },
            &EncodingKey::from_secret(self.jwt_secret()?),
        )
        .map_err(ApiError::internal)?;
        let mut authorization_url = Url::parse(&self.authorization_url)
            .map_err(|error| ApiError::internal(format!("飞书授权地址无效：{error}")))?;
        authorization_url
            .query_pairs_mut()
            .append_pair("app_id", &app.app_id)
            .append_pair("redirect_uri", &redirect_uri)
            .append_pair("state", &state);
        Ok(AuthorizeResponse {
            authorization_url: authorization_url.into(),
            state,
            expires_at,
        })
    }

    pub async fn callback(&self, request: CallbackRequest) -> Result<LoginResponse, ApiError> {
        let code = request.code.trim();
        if code.is_empty() || request.state.trim().is_empty() {
            return Err(ApiError::bad_request("code 和 state 均不能为空"));
        }
        let state = self.decode_state(request.state.trim())?;
        let app = self.resolve_login_app(state.feishu_app_id).await?;
        let user_access_token = self.exchange_code(&app, code, &state.redirect_uri).await?;
        let user = self.load_feishu_user(&user_access_token).await?;
        if user.union_id.trim().is_empty() {
            return Err(ApiError::bad_gateway("飞书没有返回 union_id"));
        }
        self.issue_session(
            &app,
            LoginUserDto {
                union_id: user.union_id,
                open_id: user.open_id,
                name: user.name,
                avatar_url: user.avatar_url,
            },
        )
        .await
    }

    async fn issue_session(
        &self,
        app: &FeishuLoginApp,
        user: LoginUserDto,
    ) -> Result<LoginResponse, ApiError> {
        let issued_at = Utc::now();
        let expires_at = issued_at + Duration::days(SESSION_LIFETIME_DAYS);
        let session_id = random_token(32)?;
        sqlx::query(
            r#"
            INSERT INTO auth_session (
                session_id, union_id, open_id, user_name, avatar_url,
                feishu_app_id, is_default_app, issued_at, expires_at
            ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)
            "#,
        )
        .bind(&session_id)
        .bind(user.union_id.trim())
        .bind(user.open_id.trim())
        .bind(user.name.trim())
        .bind(user.avatar_url.as_deref())
        .bind(app.feishu_app_id)
        .bind(app.is_default)
        .bind(issued_at)
        .bind(expires_at)
        .execute(&self.pool)
        .await?;
        let claims = LoginClaims {
            sub: user.union_id.clone(),
            jti: session_id,
            iss: self.issuer.clone(),
            aud: SESSION_AUDIENCE.to_owned(),
            iat: issued_at.timestamp(),
            exp: expires_at.timestamp(),
            feishu_app_id: app.feishu_app_id,
            is_default_app: app.is_default,
        };
        let access_token = encode(
            &Header::new(Algorithm::HS256),
            &claims,
            &EncodingKey::from_secret(self.jwt_secret()?),
        )
        .map_err(ApiError::internal)?;
        let project_permissions = self.load_project_permissions(app.feishu_app_id).await?;
        Ok(LoginResponse {
            access_token,
            token_type: "Bearer".to_owned(),
            expires_in: Duration::days(SESSION_LIFETIME_DAYS).num_seconds(),
            expires_at,
            is_default_app: app.is_default,
            feishu_app_id: app.feishu_app_id,
            user,
            project_permissions,
        })
    }

    pub async fn login_with_app_token(
        &self,
        request: AppTokenLoginRequest,
    ) -> Result<LoginResponse, ApiError> {
        let token = normalize_app_token(&request.token)?;
        let token_hash = app_token_hash(token);
        let row = sqlx::query(
            r#"
            UPDATE xingtu_feishu_app_login_token login_token
            SET last_used_at = now()
            FROM xingtu_feishu_app app
            WHERE login_token.token_hash = $1
                AND login_token.is_active = true
                AND app.feishu_app_id = login_token.feishu_app_id
                AND app.is_active = true
            RETURNING app.feishu_app_id, app.display_name
            "#,
        )
        .bind(token_hash.as_slice())
        .fetch_optional(&self.pool)
        .await?
        .ok_or_else(|| ApiError::forbidden("应用 Token 无效或已撤销"))?;
        let feishu_app_id: i64 = row.try_get("feishu_app_id")?;
        if self.project_apps.default_database_app_id() == Some(feishu_app_id) {
            return Err(ApiError::forbidden("默认飞书应用不能使用应用 Token 登录"));
        }
        let display_name: String = row.try_get("display_name")?;
        let app = self
            .project_apps
            .login_app(feishu_app_id)
            .await
            .map_err(ApiError::internal)?
            .ok_or_else(|| ApiError::forbidden("应用 Token 对应的飞书应用不可用"))?;
        let identity = format!("app-token:{feishu_app_id}");
        self.issue_session(
            &FeishuLoginApp {
                is_default: false,
                ..app
            },
            LoginUserDto {
                union_id: identity.clone(),
                open_id: identity,
                name: format!("{display_name}（应用 Token）"),
                avatar_url: None,
            },
        )
        .await
    }

    pub async fn app_login_token_status(
        &self,
        feishu_app_id: i64,
    ) -> Result<AppLoginTokenDto, ApiError> {
        self.ensure_non_default_app(feishu_app_id).await?;
        let row = sqlx::query(
            r#"
            SELECT token_prefix, remark, is_active, created_at, updated_at, last_used_at
            FROM xingtu_feishu_app_login_token
            WHERE feishu_app_id = $1
            "#,
        )
        .bind(feishu_app_id)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row
            .map(app_login_token_from_row)
            .unwrap_or_else(|| AppLoginTokenDto {
                configured: false,
                is_active: false,
                token_prefix: None,
                remark: None,
                created_at: None,
                updated_at: None,
                last_used_at: None,
            }))
    }

    pub async fn rotate_app_login_token(
        &self,
        feishu_app_id: i64,
        remark: Option<String>,
    ) -> Result<RotatedAppLoginTokenDto, ApiError> {
        self.ensure_non_default_app(feishu_app_id).await?;
        let token = format!("sk-{}", random_token(32)?);
        let token_hash = app_token_hash(&token);
        let prefix = token.chars().take(11).collect::<String>();
        let remark = normalize_remark(remark)?;
        let row = sqlx::query(
            r#"
            INSERT INTO xingtu_feishu_app_login_token (
                feishu_app_id, token_hash, token_prefix, remark, is_active
            ) VALUES ($1, $2, $3, $4, true)
            ON CONFLICT (feishu_app_id) DO UPDATE SET
                token_hash = EXCLUDED.token_hash,
                token_prefix = EXCLUDED.token_prefix,
                remark = EXCLUDED.remark,
                is_active = true,
                last_used_at = NULL
            RETURNING token_prefix, remark, is_active, created_at, updated_at, last_used_at
            "#,
        )
        .bind(feishu_app_id)
        .bind(token_hash.as_slice())
        .bind(prefix)
        .bind(remark)
        .fetch_one(&self.pool)
        .await?;
        Ok(RotatedAppLoginTokenDto {
            status: app_login_token_from_row(row),
            token,
        })
    }

    pub async fn update_app_login_token_remark(
        &self,
        feishu_app_id: i64,
        remark: Option<String>,
    ) -> Result<AppLoginTokenDto, ApiError> {
        self.ensure_non_default_app(feishu_app_id).await?;
        let remark = normalize_remark(remark)?;
        let row = sqlx::query(
            r#"
            UPDATE xingtu_feishu_app_login_token SET remark = $2
            WHERE feishu_app_id = $1
            RETURNING token_prefix, remark, is_active, created_at, updated_at, last_used_at
            "#,
        )
        .bind(feishu_app_id)
        .bind(remark)
        .fetch_optional(&self.pool)
        .await?
        .ok_or_else(|| ApiError::not_found("该应用尚未生成登录 Token"))?;
        Ok(app_login_token_from_row(row))
    }

    pub async fn revoke_app_login_token(&self, feishu_app_id: i64) -> Result<(), ApiError> {
        self.ensure_non_default_app(feishu_app_id).await?;
        let result = sqlx::query(
            "UPDATE xingtu_feishu_app_login_token SET is_active = false WHERE feishu_app_id = $1",
        )
        .bind(feishu_app_id)
        .execute(&self.pool)
        .await?;
        if result.rows_affected() == 0 {
            return Err(ApiError::not_found("该应用尚未生成登录 Token"));
        }
        Ok(())
    }

    async fn ensure_non_default_app(&self, feishu_app_id: i64) -> Result<(), ApiError> {
        if feishu_app_id <= 0 {
            return Err(ApiError::bad_request("feishu_app_id 必须大于 0"));
        }
        if self.project_apps.default_database_app_id() == Some(feishu_app_id) {
            return Err(ApiError::bad_request("默认飞书应用不能配置应用 Token"));
        }
        self.project_apps
            .get(feishu_app_id)
            .await
            .map_err(ApiError::internal)?
            .ok_or_else(|| ApiError::not_found("飞书应用不存在"))?;
        Ok(())
    }

    fn decode_state(&self, state: &str) -> Result<LoginStateClaims, ApiError> {
        let mut validation = Validation::new(Algorithm::HS256);
        validation.set_audience(&[STATE_AUDIENCE]);
        validation.set_issuer(&[self.issuer.as_str()]);
        decode::<LoginStateClaims>(
            state,
            &DecodingKey::from_secret(self.jwt_secret()?),
            &validation,
        )
        .map(|data| data.claims)
        .map_err(|_| ApiError::bad_request("登录 state 无效或已过期"))
    }

    pub async fn authenticate_jwt(&self, token: &str) -> Result<AuthenticatedSession, ApiError> {
        let mut validation = Validation::new(Algorithm::HS256);
        validation.set_audience(&[SESSION_AUDIENCE]);
        validation.set_issuer(&[self.issuer.as_str()]);
        let claims = decode::<LoginClaims>(
            token,
            &DecodingKey::from_secret(self.jwt_secret()?),
            &validation,
        )
        .map_err(|_| ApiError::forbidden("登录已失效，请重新登录"))?
        .claims;
        let row = sqlx::query(
            r#"
            UPDATE auth_session
            SET last_seen_at = now()
            WHERE session_id = $1 AND union_id = $2
                AND is_default_app = $3
                AND feishu_app_id IS NOT DISTINCT FROM $4
                AND revoked_at IS NULL AND expires_at > now()
            RETURNING session_id, union_id, open_id, user_name, avatar_url,
                feishu_app_id, is_default_app, expires_at
            "#,
        )
        .bind(&claims.jti)
        .bind(&claims.sub)
        .bind(claims.is_default_app)
        .bind(claims.feishu_app_id)
        .fetch_optional(&self.pool)
        .await?
        .ok_or_else(|| ApiError::forbidden("登录会话已撤销或过期"))?;
        let feishu_app_id: Option<i64> = row.try_get("feishu_app_id")?;
        Ok(AuthenticatedSession {
            session_id: row.try_get("session_id")?,
            union_id: row.try_get("union_id")?,
            open_id: row.try_get("open_id")?,
            user_name: row.try_get("user_name")?,
            avatar_url: row.try_get("avatar_url")?,
            feishu_app_id,
            is_default_app: row.try_get("is_default_app")?,
            expires_at: row.try_get("expires_at")?,
            project_permissions: self.load_project_permissions(feishu_app_id).await?,
        })
    }

    pub async fn revoke(&self, session_id: &str) -> Result<(), ApiError> {
        sqlx::query("UPDATE auth_session SET revoked_at = now() WHERE session_id = $1")
            .bind(session_id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    async fn load_project_permissions(
        &self,
        feishu_app_id: Option<i64>,
    ) -> Result<Vec<ProjectPermission>, ApiError> {
        let Some(feishu_app_id) = feishu_app_id else {
            return Ok(Vec::new());
        };
        let rows = sqlx::query(
            r#"
            SELECT project_id, can_view, can_manage
            FROM xingtu_feishu_app_project_access
            WHERE feishu_app_id = $1 AND can_view = true
            ORDER BY project_id
            "#,
        )
        .bind(feishu_app_id)
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter()
            .map(|row| {
                Ok(ProjectPermission {
                    project_id: row.try_get("project_id")?,
                    can_view: row.try_get("can_view")?,
                    can_manage: row.try_get("can_manage")?,
                })
            })
            .collect::<Result<_, sqlx::Error>>()
            .map_err(Into::into)
    }

    async fn exchange_code(
        &self,
        app: &FeishuLoginApp,
        code: &str,
        redirect_uri: &str,
    ) -> Result<String, ApiError> {
        let response = self
            .http
            .post(format!(
                "{}/open-apis/authen/v2/oauth/token",
                self.openapi_base_url
            ))
            .json(&serde_json::json!({
                "grant_type": "authorization_code",
                "client_id": app.app_id,
                "client_secret": app.app_secret,
                "code": code,
                "redirect_uri": redirect_uri,
            }))
            .send()
            .await
            .map_err(ApiError::bad_gateway)?;
        let status = response.status();
        let body: FeishuTokenResponse = response.json().await.map_err(ApiError::bad_gateway)?;
        if !status.is_success() || body.code.is_some_and(|code| code != 0) {
            return Err(ApiError::bad_gateway(format!(
                "飞书登录换取 user_access_token 失败：{}",
                body.error_description
                    .or(body.msg)
                    .or(body.error)
                    .unwrap_or_else(|| status.to_string())
            )));
        }
        body.access_token
            .filter(|token| !token.trim().is_empty())
            .ok_or_else(|| ApiError::bad_gateway("飞书没有返回 user_access_token"))
    }

    async fn load_feishu_user(&self, access_token: &str) -> Result<FeishuUserInfo, ApiError> {
        let response = self
            .http
            .get(format!(
                "{}/open-apis/authen/v1/user_info",
                self.openapi_base_url
            ))
            .bearer_auth(access_token)
            .send()
            .await
            .map_err(ApiError::bad_gateway)?;
        let status = response.status();
        let body: FeishuUserEnvelope = response.json().await.map_err(ApiError::bad_gateway)?;
        if !status.is_success() || body.code != 0 {
            return Err(ApiError::bad_gateway(format!(
                "飞书获取登录用户信息失败：{}",
                body.msg
            )));
        }
        body.data
            .ok_or_else(|| ApiError::bad_gateway("飞书没有返回登录用户信息"))
    }
}

pub fn routes() -> Router {
    Router::with_path("auth")
        .push(Router::with_path("apps").get(list_login_apps))
        .push(Router::with_path("authorize").post(authorize))
        .push(Router::with_path("callback").post(callback))
        .push(Router::with_path("app-token").post(login_with_app_token))
        .push(
            Router::with_path("me")
                .hoop(crate::server::auth::require_data_access)
                .get(me),
        )
        .push(
            Router::with_path("logout")
                .hoop(crate::server::auth::require_login_session)
                .post(logout),
        )
}

#[endpoint(tags("auth"), summary = "列出可选飞书登录应用")]
async fn list_login_apps(depot: &mut Depot) -> ApiResult<Vec<LoginAppDto>> {
    let state = state_from_depot(depot)?;
    Ok(Json(state.login.list_login_apps().await?))
}

#[endpoint(tags("auth"), summary = "生成飞书登录授权地址")]
async fn authorize(
    body: RequiredJsonBody<AuthorizeRequest>,
    depot: &mut Depot,
) -> ApiResult<AuthorizeResponse> {
    let state = state_from_depot(depot)?;
    Ok(Json(state.login.authorize(body.into_inner()).await?))
}

#[endpoint(tags("auth"), summary = "使用飞书授权码登录并签发七天 JWT")]
async fn callback(
    body: RequiredJsonBody<CallbackRequest>,
    depot: &mut Depot,
) -> ApiResult<LoginResponse> {
    let state = state_from_depot(depot)?;
    Ok(Json(state.login.callback(body.into_inner()).await?))
}

#[endpoint(
    tags("auth"),
    summary = "使用非默认应用的长期 Token 登录",
    description = "校验默认管理员为非默认飞书应用生成的 sk-... Token，并签发有效期七天的 JWT。应用 Token 本身不会自动过期，只有轮换或撤销后失效；JWT 权限实时读取该应用的项目访问配置。"
)]
async fn login_with_app_token(
    body: RequiredJsonBody<AppTokenLoginRequest>,
    depot: &mut Depot,
    res: &mut Response,
) -> ApiResult<LoginResponse> {
    let state = state_from_depot(depot)?;
    res.headers_mut().insert(
        CACHE_CONTROL,
        HeaderValue::from_static("no-store, max-age=0"),
    );
    res.headers_mut()
        .insert(PRAGMA, HeaderValue::from_static("no-cache"));
    Ok(Json(
        state.login.login_with_app_token(body.into_inner()).await?,
    ))
}

#[endpoint(tags("auth"), summary = "查询当前登录用户和项目权限")]
async fn me(depot: &mut Depot) -> ApiResult<MeResponse> {
    let actor = crate::server::auth::actor_from_depot(depot)?;
    let session = actor
        .session()
        .ok_or_else(|| ApiError::forbidden("内部 API Token 没有用户资料"))?;
    Ok(Json(MeResponse {
        is_default_app: session.is_default_app,
        feishu_app_id: session.feishu_app_id,
        expires_at: session.expires_at,
        user: LoginUserDto {
            union_id: session.union_id.clone(),
            open_id: session.open_id.clone(),
            name: session.user_name.clone(),
            avatar_url: session.avatar_url.clone(),
        },
        project_permissions: session.project_permissions.clone(),
    }))
}

#[endpoint(tags("auth"), summary = "退出并立即撤销当前 JWT")]
async fn logout(depot: &mut Depot) -> ApiResult<serde_json::Value> {
    let state = state_from_depot(depot)?;
    let actor = crate::server::auth::actor_from_depot(depot)?;
    let session = actor
        .session()
        .ok_or_else(|| ApiError::forbidden("当前不是用户登录会话"))?;
    state.login.revoke(&session.session_id).await?;
    Ok(Json(serde_json::json!({ "ok": true })))
}

fn normalized_redirect_uri(value: &str) -> Result<String, ApiError> {
    let value = value.trim();
    let url = Url::parse(value).map_err(|_| ApiError::bad_request("redirect_uri 格式无效"))?;
    if !matches!(url.scheme(), "https" | "http") || url.fragment().is_some() {
        return Err(ApiError::bad_request(
            "redirect_uri 只支持 http/https 且不能包含 fragment",
        ));
    }
    Ok(url.into())
}

fn parse_redirect_uris(value: Option<&str>) -> anyhow::Result<HashSet<String>> {
    let Some(value) = value.map(str::trim).filter(|value| !value.is_empty()) else {
        return Ok(HashSet::new());
    };
    let values = if value.starts_with('[') {
        serde_json::from_str::<Vec<String>>(value)
            .map_err(|error| anyhow::anyhow!("AUTH_REDIRECT_URIS JSON 无效：{error}"))?
    } else {
        value.split(',').map(str::trim).map(str::to_owned).collect()
    };
    values
        .into_iter()
        .map(|value| normalized_redirect_uri(&value).map_err(|error| anyhow::anyhow!("{error:?}")))
        .collect()
}

fn random_token(length: usize) -> Result<String, ApiError> {
    let mut bytes = vec![0_u8; length];
    getrandom::fill(&mut bytes).map_err(ApiError::internal)?;
    Ok(URL_SAFE_NO_PAD.encode(bytes))
}

fn normalize_app_token(value: &str) -> Result<&str, ApiError> {
    let value = value.trim().strip_prefix("Bearer ").unwrap_or(value.trim());
    if !value.starts_with("sk-") || value.len() < 32 || value.len() > 128 {
        return Err(ApiError::forbidden("应用 Token 无效或已撤销"));
    }
    Ok(value)
}

fn app_token_hash(token: &str) -> [u8; 32] {
    Sha256::digest(token.as_bytes()).into()
}

fn normalize_remark(value: Option<String>) -> Result<Option<String>, ApiError> {
    let value = value
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty());
    if value
        .as_ref()
        .is_some_and(|value| value.chars().count() > 200)
    {
        return Err(ApiError::bad_request("Token 备注不能超过 200 个字符"));
    }
    Ok(value)
}

fn app_login_token_from_row(row: sqlx::postgres::PgRow) -> AppLoginTokenDto {
    AppLoginTokenDto {
        configured: true,
        is_active: row.get("is_active"),
        token_prefix: row.get("token_prefix"),
        remark: row.get("remark"),
        created_at: row.get("created_at"),
        updated_at: row.get("updated_at"),
        last_used_at: row.get("last_used_at"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redirect_uri_only_accepts_http_without_fragment() {
        assert_eq!(
            normalized_redirect_uri(" https://dashboard.example.com/login?from=feishu ").unwrap(),
            "https://dashboard.example.com/login?from=feishu"
        );
        assert!(normalized_redirect_uri("javascript:alert(1)").is_err());
        assert!(normalized_redirect_uri("https://dashboard.example.com/login#code").is_err());
    }

    #[test]
    fn redirect_uri_allowlist_supports_json_and_comma_formats() {
        let json = parse_redirect_uris(Some(
            r#"["https://a.example.com/login","http://127.0.0.1:5173/login"]"#,
        ))
        .unwrap();
        assert!(json.contains("https://a.example.com/login"));
        assert!(json.contains("http://127.0.0.1:5173/login"));

        let comma = parse_redirect_uris(Some(
            "https://a.example.com/login, http://127.0.0.1:5173/login",
        ))
        .unwrap();
        assert_eq!(json, comma);
    }

    #[test]
    fn app_token_validation_and_hashing_are_stable() {
        let token = "sk-abcdefghijklmnopqrstuvwxyz0123456789";
        assert_eq!(normalize_app_token(token).unwrap(), token);
        assert_eq!(app_token_hash(token), app_token_hash(token));
        assert_ne!(
            app_token_hash(token),
            app_token_hash("sk-another-valid-token-0123456789")
        );
        assert!(normalize_app_token("ordinary-jwt").is_err());
    }

    #[test]
    fn app_token_remark_is_trimmed_and_limited() {
        assert_eq!(
            normalize_remark(Some("  上海团队  ".into())).unwrap(),
            Some("上海团队".into())
        );
        assert_eq!(normalize_remark(Some("  ".into())).unwrap(), None);
        assert!(normalize_remark(Some("x".repeat(201))).is_err());
    }

    #[test]
    fn app_token_login_endpoint_is_in_openapi() {
        let openapi =
            salvo::oapi::OpenApi::new("test", "1.0.0").merge_router_with_base(&routes(), "/api/v1");
        assert!(openapi.paths.contains_key("/api/v1/auth/app-token"));
    }
}
