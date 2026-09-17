use crate::lark::project_app::{FeishuLoginApp, ProjectFeishuAppStore};
use crate::server::api::RequiredJsonBody;
use crate::server::error::{ApiError, ApiResult};
use crate::server::state::state_from_depot;
use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use chrono::{DateTime, Duration, Utc};
use jsonwebtoken::{Algorithm, DecodingKey, EncodingKey, Header, Validation, decode, encode};
use salvo::oapi::ToSchema;
use salvo::prelude::*;
use serde::{Deserialize, Serialize};
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
            user: LoginUserDto {
                union_id: user.union_id,
                open_id: user.open_id,
                name: user.name,
                avatar_url: user.avatar_url,
            },
            project_permissions,
        })
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
}
