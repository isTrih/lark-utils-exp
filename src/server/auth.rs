use crate::server::error::{ApiErrorResponse, request_id_from_depot};
use salvo::http::header::AUTHORIZATION;
use salvo::prelude::*;
use std::env;

const AUTH_ACTOR_DEPOT_KEY: &str = "auth_actor";

#[derive(Debug, Clone)]
pub enum AuthActor {
    MutationAdmin,
    InternalQuery,
    Login(crate::server::login::AuthenticatedSession),
}

impl AuthActor {
    pub fn session(&self) -> Option<&crate::server::login::AuthenticatedSession> {
        match self {
            Self::Login(session) => Some(session),
            _ => None,
        }
    }

    pub fn is_default_admin(&self) -> bool {
        matches!(self, Self::MutationAdmin)
            || matches!(self, Self::Login(session) if session.is_default_app)
    }

    pub fn can_query_all(&self) -> bool {
        self.is_default_admin() || matches!(self, Self::InternalQuery)
    }

    pub fn can_view_project(&self, project_id: i64) -> bool {
        self.can_query_all()
            || matches!(self, Self::Login(session) if session.project_permissions.iter().any(
                |permission| permission.project_id == project_id && permission.can_view
            ))
    }

    pub fn can_manage_project(&self, project_id: i64) -> bool {
        self.is_default_admin()
            || matches!(self, Self::Login(session) if session.project_permissions.iter().any(
                |permission| permission.project_id == project_id && permission.can_manage
            ))
    }

    pub fn visible_project_ids(&self) -> Option<Vec<i64>> {
        match self {
            Self::Login(session) if !session.is_default_app => Some(
                session
                    .project_permissions
                    .iter()
                    .filter(|permission| permission.can_view)
                    .map(|permission| permission.project_id)
                    .collect(),
            ),
            _ => None,
        }
    }
}

const MUTATION_TOKEN_ENV: &str = "MUTATION_API_TOKEN";
const ADMIN_TOKEN_ENV: &str = "ADMIN_API_TOKEN";
const XINGTU_SESSION_UPLOAD_TOKEN_ENV: &str = "XINGTU_SESSION_UPLOAD_TOKEN";

/// 内部写接口的简单 Bearer token 鉴权。
///
/// 优先读取 `MUTATION_API_TOKEN`，未配置时兼容复用 `ADMIN_API_TOKEN`。
#[handler]
pub async fn require_mutation_token(
    req: &mut Request,
    depot: &mut Depot,
    res: &mut Response,
    ctrl: &mut FlowCtrl,
) {
    let expected = configured_mutation_token();
    enforce_mutation_token(req, depot, res, ctrl, expected.as_deref()).await;
}

/// 星图浏览器插件专用鉴权。
///
/// 插件共享的 `XINGTU_SESSION_UPLOAD_TOKEN` 只能挂在登录态上传路由；管理员 token 继续兼容
/// 运维调用，但插件包不需要也不应该携带管理员 token。
#[handler]
pub async fn require_xingtu_session_upload_token(
    req: &mut Request,
    depot: &mut Depot,
    res: &mut Response,
    ctrl: &mut FlowCtrl,
) {
    let upload_token = configured_xingtu_session_upload_token();
    let mutation_token = configured_mutation_token();
    enforce_any_token(
        req,
        depot,
        res,
        ctrl,
        &[upload_token.as_deref(), mutation_token.as_deref()],
        "服务端未配置 XINGTU_SESSION_UPLOAD_TOKEN、MUTATION_API_TOKEN 或 ADMIN_API_TOKEN",
        "星图登录态上传鉴权失败",
    )
    .await;
}

async fn enforce_mutation_token(
    req: &mut Request,
    depot: &mut Depot,
    res: &mut Response,
    ctrl: &mut FlowCtrl,
    expected: Option<&str>,
) {
    enforce_any_token(
        req,
        depot,
        res,
        ctrl,
        &[expected],
        "服务端未配置 MUTATION_API_TOKEN 或 ADMIN_API_TOKEN",
        "内部写接口鉴权失败",
    )
    .await;
}

async fn enforce_any_token(
    req: &mut Request,
    depot: &mut Depot,
    res: &mut Response,
    ctrl: &mut FlowCtrl,
    expected_tokens: &[Option<&str>],
    unavailable_message: &str,
    failure_log: &str,
) {
    if expected_tokens.iter().all(Option::is_none) {
        reject(
            depot,
            res,
            StatusCode::SERVICE_UNAVAILABLE,
            "auth_unavailable",
            unavailable_message,
        );
        ctrl.skip_rest();
        return;
    }
    let provided = req
        .headers()
        .get(AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .unwrap_or_default();
    let accepted = expected_tokens
        .iter()
        .flatten()
        .any(|expected| tokens_equal(provided, expected));
    if !accepted {
        tracing::warn!(
            request_id = %request_id_from_depot(depot),
            method = %req.method(),
            path = %req.uri().path(),
            "{failure_log}"
        );
        reject(
            depot,
            res,
            StatusCode::UNAUTHORIZED,
            "unauthorized",
            "Bearer token 无效",
        );
        ctrl.skip_rest();
        return;
    }

    ctrl.call_next(req, depot, res).await;
}

pub(crate) fn configured_mutation_token() -> Option<String> {
    [MUTATION_TOKEN_ENV, ADMIN_TOKEN_ENV]
        .into_iter()
        .find_map(|name| {
            env::var(name)
                .ok()
                .map(|value| value.trim().to_owned())
                .filter(|value| !value.is_empty())
        })
}

pub fn actor_from_depot(depot: &Depot) -> Result<&AuthActor, crate::server::error::ApiError> {
    depot
        .get::<AuthActor>(AUTH_ACTOR_DEPOT_KEY)
        .map_err(|_| crate::server::error::ApiError::forbidden("当前请求尚未通过身份认证"))
}

pub fn require_project_view(
    depot: &Depot,
    project_id: i64,
) -> Result<(), crate::server::error::ApiError> {
    if actor_from_depot(depot)?.can_view_project(project_id) {
        Ok(())
    } else {
        Err(crate::server::error::ApiError::forbidden(
            "当前登录应用无权查看该项目",
        ))
    }
}

pub fn require_project_manage(
    depot: &Depot,
    project_id: i64,
) -> Result<(), crate::server::error::ApiError> {
    if actor_from_depot(depot)?.can_manage_project(project_id) {
        Ok(())
    } else {
        Err(crate::server::error::ApiError::forbidden(
            "当前登录应用无权配置该项目",
        ))
    }
}

pub fn require_global_query(depot: &Depot) -> Result<(), crate::server::error::ApiError> {
    if actor_from_depot(depot)?.can_query_all() {
        Ok(())
    } else {
        Err(crate::server::error::ApiError::forbidden(
            "非默认应用调用该接口时必须明确限定已授权项目",
        ))
    }
}

pub async fn require_period_view(
    depot: &Depot,
    pool: &sqlx::PgPool,
    activity_period_id: i64,
) -> Result<i64, crate::server::error::ApiError> {
    let project_id = sqlx::query_scalar::<_, i64>(
        "SELECT project_id FROM xingtu_activity_period WHERE activity_period_id = $1",
    )
    .bind(activity_period_id)
    .fetch_optional(pool)
    .await?
    .ok_or_else(|| crate::server::error::ApiError::not_found("活动期次不存在"))?;
    require_project_view(depot, project_id)?;
    Ok(project_id)
}

pub async fn require_period_manage(
    depot: &Depot,
    pool: &sqlx::PgPool,
    activity_period_id: i64,
) -> Result<i64, crate::server::error::ApiError> {
    let project_id = sqlx::query_scalar::<_, i64>(
        "SELECT project_id FROM xingtu_activity_period WHERE activity_period_id = $1",
    )
    .bind(activity_period_id)
    .fetch_optional(pool)
    .await?
    .ok_or_else(|| crate::server::error::ApiError::not_found("活动期次不存在"))?;
    require_project_manage(depot, project_id)?;
    Ok(project_id)
}

pub async fn require_content_view(
    depot: &Depot,
    pool: &sqlx::PgPool,
    content_config_id: i64,
) -> Result<i64, crate::server::error::ApiError> {
    let project_id = sqlx::query_scalar::<_, i64>(
        r#"
        SELECT period.project_id
        FROM xingtu_activity_content_config content
        JOIN xingtu_activity_period period
            ON period.activity_period_id = content.activity_period_id
        WHERE content.content_config_id = $1
        "#,
    )
    .bind(content_config_id)
    .fetch_optional(pool)
    .await?
    .ok_or_else(|| crate::server::error::ApiError::not_found("内容配置不存在"))?;
    require_project_view(depot, project_id)?;
    Ok(project_id)
}

#[handler]
pub async fn require_data_access(
    req: &mut Request,
    depot: &mut Depot,
    res: &mut Response,
    ctrl: &mut FlowCtrl,
) {
    authenticate(req, depot, res, ctrl, false, false).await;
}

#[handler]
pub async fn require_management_access(
    req: &mut Request,
    depot: &mut Depot,
    res: &mut Response,
    ctrl: &mut FlowCtrl,
) {
    authenticate(req, depot, res, ctrl, true, false).await;
}

#[handler]
pub async fn require_default_admin(
    req: &mut Request,
    depot: &mut Depot,
    res: &mut Response,
    ctrl: &mut FlowCtrl,
) {
    authenticate(req, depot, res, ctrl, true, true).await;
}

#[handler]
pub async fn require_login_session(
    req: &mut Request,
    depot: &mut Depot,
    res: &mut Response,
    ctrl: &mut FlowCtrl,
) {
    let Some(token) = bearer_token(req) else {
        reject(
            depot,
            res,
            StatusCode::UNAUTHORIZED,
            "unauthorized",
            "缺少 Bearer JWT",
        );
        ctrl.skip_rest();
        return;
    };
    let state = match crate::server::state::state_from_depot(depot) {
        Ok(state) => state,
        Err(error) => {
            reject(
                depot,
                res,
                StatusCode::INTERNAL_SERVER_ERROR,
                "internal_error",
                &error.to_string(),
            );
            ctrl.skip_rest();
            return;
        }
    };
    match state.login.authenticate_jwt(token).await {
        Ok(session) => {
            depot.insert(AUTH_ACTOR_DEPOT_KEY, AuthActor::Login(session));
            ctrl.call_next(req, depot, res).await;
        }
        Err(_) => {
            reject(
                depot,
                res,
                StatusCode::UNAUTHORIZED,
                "unauthorized",
                "登录已失效，请重新登录",
            );
            ctrl.skip_rest();
        }
    }
}

#[handler]
pub async fn require_default_actor(
    req: &mut Request,
    depot: &mut Depot,
    res: &mut Response,
    ctrl: &mut FlowCtrl,
) {
    if actor_from_depot(depot).is_ok_and(AuthActor::is_default_admin) {
        ctrl.call_next(req, depot, res).await;
    } else {
        reject(
            depot,
            res,
            StatusCode::FORBIDDEN,
            "forbidden",
            "该操作仅允许默认飞书应用管理员执行",
        );
        ctrl.skip_rest();
    }
}

#[handler]
pub async fn require_project_route_access(
    req: &mut Request,
    depot: &mut Depot,
    res: &mut Response,
    ctrl: &mut FlowCtrl,
) {
    let Some(project_id) = req.param::<i64>("project_id") else {
        reject(
            depot,
            res,
            StatusCode::BAD_REQUEST,
            "bad_request",
            "project_id 无效",
        );
        ctrl.skip_rest();
        return;
    };
    let allowed = if req.method() == salvo::http::Method::GET {
        actor_from_depot(depot).is_ok_and(|actor| actor.can_view_project(project_id))
    } else {
        actor_from_depot(depot).is_ok_and(|actor| actor.can_manage_project(project_id))
    };
    if allowed {
        ctrl.call_next(req, depot, res).await;
    } else {
        reject(
            depot,
            res,
            StatusCode::FORBIDDEN,
            "forbidden",
            "当前登录应用没有该项目权限",
        );
        ctrl.skip_rest();
    }
}

async fn authenticate(
    req: &mut Request,
    depot: &mut Depot,
    res: &mut Response,
    ctrl: &mut FlowCtrl,
    management: bool,
    default_only: bool,
) {
    let Some(provided) = bearer_token(req) else {
        reject(
            depot,
            res,
            StatusCode::UNAUTHORIZED,
            "unauthorized",
            "请先通过飞书登录",
        );
        ctrl.skip_rest();
        return;
    };
    if configured_mutation_token()
        .as_deref()
        .is_some_and(|expected| tokens_equal(provided, expected))
    {
        depot.insert(AUTH_ACTOR_DEPOT_KEY, AuthActor::MutationAdmin);
        ctrl.call_next(req, depot, res).await;
        return;
    }
    let state = match crate::server::state::state_from_depot(depot) {
        Ok(state) => state,
        Err(error) => {
            reject(
                depot,
                res,
                StatusCode::INTERNAL_SERVER_ERROR,
                "internal_error",
                &error.to_string(),
            );
            ctrl.skip_rest();
            return;
        }
    };
    if !management
        && state
            .login
            .internal_api_token()
            .is_some_and(|expected| tokens_equal(provided, expected))
    {
        depot.insert(AUTH_ACTOR_DEPOT_KEY, AuthActor::InternalQuery);
        ctrl.call_next(req, depot, res).await;
        return;
    }
    match state.login.authenticate_jwt(provided).await {
        Ok(session) if !default_only || session.is_default_app => {
            depot.insert(AUTH_ACTOR_DEPOT_KEY, AuthActor::Login(session));
            ctrl.call_next(req, depot, res).await;
        }
        Ok(_) => {
            reject(
                depot,
                res,
                StatusCode::FORBIDDEN,
                "forbidden",
                "该操作仅允许默认飞书应用管理员执行",
            );
            ctrl.skip_rest();
        }
        Err(_) => {
            reject(
                depot,
                res,
                StatusCode::UNAUTHORIZED,
                "unauthorized",
                "Bearer token 无效或已过期",
            );
            ctrl.skip_rest();
        }
    }
}

fn bearer_token(req: &Request) -> Option<&str> {
    req.headers()
        .get(AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .map(str::trim)
        .filter(|value| !value.is_empty())
}

pub(crate) fn configured_xingtu_session_upload_token() -> Option<String> {
    env::var(XINGTU_SESSION_UPLOAD_TOKEN_ENV)
        .ok()
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
}

fn reject(depot: &Depot, res: &mut Response, status: StatusCode, code: &str, message: &str) {
    res.status_code(status);
    res.render(Json(ApiErrorResponse {
        ok: false,
        code: code.to_owned(),
        message: message.to_owned(),
        request_id: request_id_from_depot(depot),
    }));
}

pub(crate) fn tokens_equal(left: &str, right: &str) -> bool {
    if left.len() != right.len() {
        return false;
    }
    left.bytes()
        .zip(right.bytes())
        .fold(0_u8, |difference, (left, right)| {
            difference | (left ^ right)
        })
        == 0
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{Duration, Utc};
    use salvo::test::{ResponseExt, TestClient};
    use serde_json::Value;

    #[handler]
    async fn fixed_auth(
        req: &mut Request,
        depot: &mut Depot,
        res: &mut Response,
        ctrl: &mut FlowCtrl,
    ) {
        enforce_mutation_token(req, depot, res, ctrl, Some("integration-secret")).await;
    }

    #[handler]
    async fn fixed_upload_auth(
        req: &mut Request,
        depot: &mut Depot,
        res: &mut Response,
        ctrl: &mut FlowCtrl,
    ) {
        enforce_any_token(
            req,
            depot,
            res,
            ctrl,
            &[Some("shared-upload-secret"), Some("admin-secret")],
            "missing",
            "test upload auth failed",
        )
        .await;
    }

    #[handler]
    async fn protected_probe() -> Json<Value> {
        Json(serde_json::json!({ "ok": true }))
    }

    #[test]
    fn token_comparison_checks_every_byte() {
        assert!(tokens_equal("same-token", "same-token"));
        assert!(!tokens_equal("same-token", "same-tokem"));
        assert!(!tokens_equal("short", "longer"));
    }

    fn login_actor(
        is_default_app: bool,
        project_permissions: Vec<crate::server::login::ProjectPermission>,
    ) -> AuthActor {
        AuthActor::Login(crate::server::login::AuthenticatedSession {
            session_id: "session".to_owned(),
            union_id: "union".to_owned(),
            open_id: "open".to_owned(),
            user_name: "测试用户".to_owned(),
            avatar_url: None,
            feishu_app_id: (!is_default_app).then_some(1),
            is_default_app,
            expires_at: Utc::now() + Duration::days(7),
            project_permissions,
        })
    }

    #[test]
    fn default_and_internal_actors_follow_global_permission_matrix() {
        let default_actor = login_actor(true, Vec::new());
        assert!(default_actor.can_query_all());
        assert!(default_actor.can_view_project(99));
        assert!(default_actor.can_manage_project(99));

        let internal = AuthActor::InternalQuery;
        assert!(internal.can_query_all());
        assert!(internal.can_view_project(99));
        assert!(!internal.can_manage_project(99));
    }

    #[test]
    fn non_default_actor_is_limited_to_explicit_project_permissions() {
        let actor = login_actor(
            false,
            vec![crate::server::login::ProjectPermission {
                project_id: 10,
                can_view: true,
                can_manage: false,
            }],
        );
        assert!(!actor.can_query_all());
        assert!(actor.can_view_project(10));
        assert!(!actor.can_manage_project(10));
        assert!(!actor.can_view_project(11));
        assert_eq!(actor.visible_project_ids(), Some(vec![10]));
    }

    #[tokio::test]
    async fn mutation_route_requires_exact_bearer_token() {
        let service = Service::new(
            Router::new()
                .hoop(crate::server::error::request_context)
                .hoop(fixed_auth)
                .post(protected_probe),
        );
        let mut missing = TestClient::post("http://127.0.0.1/").send(&service).await;
        assert_eq!(missing.status_code, Some(StatusCode::UNAUTHORIZED));
        let missing_body = missing.take_json::<Value>().await.unwrap();
        assert_eq!(missing_body["code"], "unauthorized");
        assert!(
            missing_body["request_id"]
                .as_str()
                .is_some_and(|id| !id.is_empty())
        );

        let accepted = TestClient::post("http://127.0.0.1/")
            .add_header("Authorization", "Bearer integration-secret", true)
            .send(&service)
            .await;
        assert_eq!(accepted.status_code, Some(StatusCode::OK));
    }

    #[tokio::test]
    async fn session_upload_accepts_shared_or_admin_token_only() {
        let service = Service::new(
            Router::new()
                .hoop(crate::server::error::request_context)
                .hoop(fixed_upload_auth)
                .post(protected_probe),
        );
        for token in ["shared-upload-secret", "admin-secret"] {
            let response = TestClient::post("http://127.0.0.1/")
                .add_header("Authorization", format!("Bearer {token}"), true)
                .send(&service)
                .await;
            assert_eq!(response.status_code, Some(StatusCode::OK));
        }
        let rejected = TestClient::post("http://127.0.0.1/")
            .add_header("Authorization", "Bearer wrong", true)
            .send(&service)
            .await;
        assert_eq!(rejected.status_code, Some(StatusCode::UNAUTHORIZED));
    }
}
