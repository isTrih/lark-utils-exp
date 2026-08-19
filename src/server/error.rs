use salvo::http::header::{HeaderName, HeaderValue};
use salvo::oapi::{Components, EndpointOutRegister, Operation, ToSchema};
use salvo::prelude::*;
use serde::Serialize;

pub const REQUEST_ID_HEADER: &str = "x-request-id";
const REQUEST_ID_DEPOT_KEY: &str = "request_id";

/// HTTP API 错误响应体。
#[derive(Debug, Serialize, ToSchema)]
pub struct ApiErrorResponse {
    pub ok: bool,
    pub code: String,
    pub message: String,
    pub request_id: String,
}

/// HTTP API 统一错误。
#[derive(Debug)]
pub struct ApiError {
    status: StatusCode,
    code: &'static str,
    message: String,
    internal_detail: Option<String>,
}

impl ApiError {
    pub fn bad_request(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::BAD_REQUEST,
            code: "bad_request",
            message: message.into(),
            internal_detail: None,
        }
    }

    pub fn not_found(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::NOT_FOUND,
            code: "not_found",
            message: message.into(),
            internal_detail: None,
        }
    }

    pub fn conflict(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::CONFLICT,
            code: "conflict",
            message: message.into(),
            internal_detail: None,
        }
    }

    pub fn service_unavailable(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::SERVICE_UNAVAILABLE,
            code: "service_unavailable",
            message: message.into(),
            internal_detail: None,
        }
    }

    pub fn bad_gateway(error: impl std::fmt::Display) -> Self {
        Self {
            status: StatusCode::BAD_GATEWAY,
            code: "upstream_error",
            message: "飞书接口暂时不可用".to_string(),
            internal_detail: Some(error.to_string()),
        }
    }

    pub fn internal(error: impl std::fmt::Display) -> Self {
        Self {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            code: "internal_error",
            message: "服务内部错误".to_string(),
            internal_detail: Some(error.to_string()),
        }
    }

    pub fn internal_with_message(
        message: impl Into<String>,
        error: impl std::fmt::Display,
    ) -> Self {
        Self {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            code: "internal_error",
            message: message.into(),
            internal_detail: Some(error.to_string()),
        }
    }
}

impl From<anyhow::Error> for ApiError {
    fn from(value: anyhow::Error) -> Self {
        if value
            .downcast_ref::<crate::workflow::WorkflowBusyError>()
            .is_some()
        {
            Self::conflict(value.to_string())
        } else {
            Self::internal(format!("{value:?}"))
        }
    }
}

impl From<sqlx::Error> for ApiError {
    fn from(value: sqlx::Error) -> Self {
        if value
            .as_database_error()
            .is_some_and(|error| error.is_unique_violation())
        {
            Self::conflict("记录已存在或唯一字段冲突")
        } else {
            Self::internal(value)
        }
    }
}

#[salvo::async_trait]
impl Writer for ApiError {
    async fn write(self, req: &mut Request, depot: &mut Depot, res: &mut Response) {
        let request_id = request_id_from_depot(depot);
        if let Some(error) = self.internal_detail.as_deref() {
            tracing::error!(
                request_id,
                method = %req.method(),
                path = %req.uri().path(),
                error,
                "HTTP API 请求处理失败"
            );
        }
        res.status_code(self.status);
        res.render(Json(ApiErrorResponse {
            ok: false,
            code: self.code.to_string(),
            message: self.message,
            request_id,
        }));
    }
}

#[handler]
pub async fn request_context(
    req: &mut Request,
    depot: &mut Depot,
    res: &mut Response,
    ctrl: &mut FlowCtrl,
) {
    let request_id = req
        .headers()
        .get(REQUEST_ID_HEADER)
        .and_then(|value| value.to_str().ok())
        .filter(|value| valid_request_id(value))
        .map(ToOwned::to_owned)
        .unwrap_or_else(generate_request_id);
    depot.insert(REQUEST_ID_DEPOT_KEY, request_id.clone());
    ctrl.call_next(req, depot, res).await;
    if let Ok(value) = HeaderValue::from_str(&request_id) {
        res.headers_mut()
            .insert(HeaderName::from_static(REQUEST_ID_HEADER), value);
    }
}

pub fn request_id_from_depot(depot: &Depot) -> String {
    depot
        .get::<String>(REQUEST_ID_DEPOT_KEY)
        .cloned()
        .unwrap_or_else(|_| generate_request_id())
}

fn valid_request_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
}

fn generate_request_id() -> String {
    let mut bytes = [0_u8; 16];
    if getrandom::fill(&mut bytes).is_err() {
        return format!("fallback-{}", chrono::Utc::now().timestamp_micros());
    }
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

impl EndpointOutRegister for ApiError {
    fn register(components: &mut Components, operation: &mut Operation) {
        let schema = ApiErrorResponse::to_schema(components);
        operation.responses.insert(
            "400",
            salvo::oapi::Response::new("请求参数错误")
                .add_content("application/json", schema.clone()),
        );
        operation.responses.insert(
            "404",
            salvo::oapi::Response::new("记录不存在")
                .add_content("application/json", schema.clone()),
        );
        operation.responses.insert(
            "409",
            salvo::oapi::Response::new("记录冲突").add_content("application/json", schema.clone()),
        );
        operation.responses.insert(
            "401",
            salvo::oapi::Response::new("Bearer token 无效")
                .add_content("application/json", schema.clone()),
        );
        operation.responses.insert(
            "503",
            salvo::oapi::Response::new("服务尚未就绪")
                .add_content("application/json", schema.clone()),
        );
        operation.responses.insert(
            "502",
            salvo::oapi::Response::new("飞书上游接口不可用")
                .add_content("application/json", schema.clone()),
        );
        operation.responses.insert(
            "500",
            salvo::oapi::Response::new("服务内部错误").add_content("application/json", schema),
        );
    }
}

pub type ApiResult<T> = Result<Json<T>, ApiError>;

#[cfg(test)]
mod tests {
    use super::*;
    use salvo::test::{ResponseExt, TestClient};
    use serde_json::Value;

    #[handler]
    async fn internal_error_probe() -> ApiResult<Value> {
        Err(ApiError::internal(
            "postgres://secret@db/internal SQL SELECT cookie",
        ))
    }

    #[test]
    fn request_id_validation_rejects_log_injection() {
        assert!(valid_request_id("client-Request_123"));
        assert!(!valid_request_id("contains space"));
        assert!(!valid_request_id("contains\nnewline"));
        assert!(!valid_request_id(&"x".repeat(65)));
    }

    #[tokio::test]
    async fn internal_error_response_is_redacted_and_correlated() {
        let service = Service::new(
            Router::new()
                .hoop(request_context)
                .get(internal_error_probe),
        );
        let mut response = TestClient::get("http://127.0.0.1/")
            .add_header("X-Request-ID", "client-request-42", true)
            .send(&service)
            .await;
        assert_eq!(
            response.status_code,
            Some(StatusCode::INTERNAL_SERVER_ERROR)
        );
        assert_eq!(
            response
                .headers()
                .get(REQUEST_ID_HEADER)
                .and_then(|value| value.to_str().ok()),
            Some("client-request-42")
        );
        let body = response.take_json::<Value>().await.unwrap();
        assert_eq!(body["code"], "internal_error");
        assert_eq!(body["message"], "服务内部错误");
        assert_eq!(body["request_id"], "client-request-42");
        assert!(!body.to_string().contains("postgres://"));
        assert!(!body.to_string().contains("cookie"));
    }
}
