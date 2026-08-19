use crate::server::error::{ApiErrorResponse, request_id_from_depot};
use salvo::http::header::AUTHORIZATION;
use salvo::prelude::*;
use std::env;

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

fn configured_mutation_token() -> Option<String> {
    [MUTATION_TOKEN_ENV, ADMIN_TOKEN_ENV]
        .into_iter()
        .find_map(|name| {
            env::var(name)
                .ok()
                .map(|value| value.trim().to_owned())
                .filter(|value| !value.is_empty())
        })
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
