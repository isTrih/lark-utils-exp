use std::env;

use aes_gcm::aead::{Aead, Payload};
use aes_gcm::{Aes256Gcm, KeyInit, Nonce};
use base64::Engine as _;
use base64::engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD};
use salvo::http::body::ResBody;
use salvo::http::header::{
    CACHE_CONTROL, CONTENT_ENCODING, CONTENT_LENGTH, CONTENT_TYPE, ETAG, VARY,
};
use salvo::http::{HeaderMap, HeaderName, HeaderValue, StatusCode};
use salvo::prelude::*;
use serde::Serialize;

pub const DATA_PROTECTION_HEADER: &str = "x-data-protection";
pub const DATA_PROTECTION_SCHEME: &str = "aes-256-gcm+zstd";
pub const PROTECTED_CONTENT_TYPE: &str = "application/vnd.lark-utils-exp.protected+json";

const DATA_PROTECTION_KEY_ENV: &str = "API_DATA_ENCRYPTION_KEY";
const DATA_PROTECTION_KEY_ID_ENV: &str = "API_DATA_ENCRYPTION_KEY_ID";
const DEFAULT_KEY_ID: &str = "primary";
const AAD: &[u8] = b"lark-utils-exp:data:v1";
const NONCE_LENGTH: usize = 12;
const ZSTD_LEVEL: i32 = 3;

#[derive(Clone)]
struct DataProtectionConfig {
    key: [u8; 32],
    key_id: String,
}

impl DataProtectionConfig {
    fn from_env() -> anyhow::Result<Option<Self>> {
        let encoded_key = match env::var(DATA_PROTECTION_KEY_ENV) {
            Ok(value) => value,
            Err(env::VarError::NotPresent) => return Ok(None),
            Err(env::VarError::NotUnicode(_)) => {
                anyhow::bail!("API_DATA_ENCRYPTION_KEY 必须是有效 UTF-8 文本")
            }
        };
        let key_id = match env::var(DATA_PROTECTION_KEY_ID_ENV) {
            Ok(value) => Some(value),
            Err(env::VarError::NotPresent) => None,
            Err(env::VarError::NotUnicode(_)) => {
                anyhow::bail!("API_DATA_ENCRYPTION_KEY_ID 必须是有效 UTF-8 文本")
            }
        };
        Self::from_values(Some(&encoded_key), key_id.as_deref()).map_err(|_| {
            anyhow::anyhow!("API_DATA_ENCRYPTION_KEY 必须是标准 Base64 编码的 32 字节密钥")
        })
    }

    fn from_values(
        encoded_key: Option<&str>,
        key_id: Option<&str>,
    ) -> Result<Option<Self>, ConfigError> {
        let Some(encoded_key) = encoded_key else {
            return Ok(None);
        };
        let key = decode_key(encoded_key)?;
        let key_id = key_id
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .unwrap_or(DEFAULT_KEY_ID)
            .to_owned();

        Ok(Some(Self { key, key_id }))
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ConfigError {
    InvalidKey,
}

#[derive(Clone)]
enum ConfigSource {
    Ready(DataProtectionConfig),
    Unavailable,
}

/// 按请求协商的 JSON 响应保护中间件。
///
/// 未携带 `X-Data-Protection` 时不会读取密钥，也不会改变响应。携带支持的协商值后，
/// 中间件会在调用下游前确认配置可用，并在响应返回时以 fail-closed 方式完成保护。
#[derive(Clone)]
pub struct DataProtection {
    config: ConfigSource,
}

impl DataProtection {
    /// 从进程环境加载配置。密钥缺失允许启动；密钥存在但无效时拒绝启动。
    pub fn from_env() -> anyhow::Result<Self> {
        let config = match DataProtectionConfig::from_env()? {
            Some(config) => ConfigSource::Ready(config),
            None => ConfigSource::Unavailable,
        };
        Ok(Self { config })
    }

    #[cfg(test)]
    fn with_config(config: DataProtectionConfig) -> Self {
        Self {
            config: ConfigSource::Ready(config),
        }
    }

    #[cfg(test)]
    pub(super) fn with_key_for_test(key: [u8; 32], key_id: &str) -> Self {
        Self::with_config(DataProtectionConfig {
            key,
            key_id: key_id.to_owned(),
        })
    }

    #[cfg(test)]
    fn unavailable() -> Self {
        Self {
            config: ConfigSource::Unavailable,
        }
    }
}

#[salvo::async_trait]
impl Handler for DataProtection {
    async fn handle(
        &self,
        req: &mut Request,
        depot: &mut Depot,
        res: &mut Response,
        ctrl: &mut FlowCtrl,
    ) {
        let mut requested_values = req.headers().get_all(DATA_PROTECTION_HEADER).iter();
        let first_requested_value = requested_values.next();
        let has_more_values = requested_values.next().is_some();
        let requested_scheme = match (first_requested_value, has_more_values) {
            (None, false) => {
                ctrl.call_next(req, depot, res).await;
                return;
            }
            (Some(value), false) if value.as_bytes() == DATA_PROTECTION_SCHEME.as_bytes() => {
                DATA_PROTECTION_SCHEME
            }
            _ => {
                render_failure(
                    res,
                    StatusCode::BAD_REQUEST,
                    "unsupported_data_protection",
                    "不支持的响应数据保护方式",
                );
                ctrl.skip_rest();
                return;
            }
        };

        let ConfigSource::Ready(config) = &self.config else {
            render_failure(
                res,
                StatusCode::SERVICE_UNAVAILABLE,
                "data_protection_unavailable",
                "响应数据保护当前不可用",
            );
            ctrl.skip_rest();
            return;
        };

        ctrl.call_next(req, depot, res).await;

        let is_json = response_is_json(res.headers());
        let original_body = res.take_body();
        let protected = collect_body(original_body)
            .and_then(|body| {
                if !is_json || serde_json::from_slice::<serde_json::Value>(&body).is_err() {
                    return Err(ProtectionError::UnsupportedBody);
                }
                protect_json(&body, config)
            })
            .and_then(|envelope| {
                serde_json::to_vec(&envelope).map_err(|_| ProtectionError::Serialization)
            });

        match protected {
            Ok(body) => render_protected(res, requested_scheme, body),
            Err(_) => render_failure(
                res,
                StatusCode::INTERNAL_SERVER_ERROR,
                "data_protection_failed",
                "响应数据保护失败",
            ),
        }
    }
}

#[derive(Debug, Eq, PartialEq)]
enum ProtectionError {
    InvalidNonce,
    Compression,
    Encryption,
    Randomness,
    Serialization,
    UnsupportedBody,
}

#[derive(Serialize)]
struct ProtectedEnvelope {
    protected: bool,
    data: ProtectedData,
}

#[derive(Serialize)]
struct ProtectedData {
    version: u8,
    scheme: &'static str,
    key_id: String,
    nonce: String,
    ciphertext: String,
}

fn decode_key(encoded: &str) -> Result<[u8; 32], ConfigError> {
    let decoded = STANDARD
        .decode(encoded)
        .map_err(|_| ConfigError::InvalidKey)?;
    decoded.try_into().map_err(|_| ConfigError::InvalidKey)
}

fn protect_json(
    original_json: &[u8],
    config: &DataProtectionConfig,
) -> Result<ProtectedEnvelope, ProtectionError> {
    let mut nonce = [0_u8; NONCE_LENGTH];
    getrandom::fill(&mut nonce).map_err(|_| ProtectionError::Randomness)?;
    protect_json_with_nonce(original_json, config, &nonce)
}

fn protect_json_with_nonce(
    original_json: &[u8],
    config: &DataProtectionConfig,
    nonce: &[u8],
) -> Result<ProtectedEnvelope, ProtectionError> {
    if nonce.len() != NONCE_LENGTH {
        return Err(ProtectionError::InvalidNonce);
    }

    let compressed = zstd::stream::encode_all(original_json, ZSTD_LEVEL)
        .map_err(|_| ProtectionError::Compression)?;
    let cipher = Aes256Gcm::new_from_slice(&config.key).map_err(|_| ProtectionError::Encryption)?;
    let ciphertext = cipher
        .encrypt(
            Nonce::from_slice(nonce),
            Payload {
                msg: &compressed,
                aad: AAD,
            },
        )
        .map_err(|_| ProtectionError::Encryption)?;

    Ok(ProtectedEnvelope {
        protected: true,
        data: ProtectedData {
            version: 1,
            scheme: DATA_PROTECTION_SCHEME,
            key_id: config.key_id.clone(),
            nonce: URL_SAFE_NO_PAD.encode(nonce),
            ciphertext: URL_SAFE_NO_PAD.encode(ciphertext),
        },
    })
}

fn collect_body(body: ResBody) -> Result<Vec<u8>, ProtectionError> {
    match body {
        ResBody::Once(bytes) => Ok(bytes.to_vec()),
        ResBody::Chunks(chunks) => {
            let capacity = chunks.iter().map(|chunk| chunk.len()).sum();
            let mut body = Vec::with_capacity(capacity);
            for chunk in chunks {
                body.extend_from_slice(&chunk);
            }
            Ok(body)
        }
        _ => Err(ProtectionError::UnsupportedBody),
    }
}

fn response_is_json(headers: &HeaderMap) -> bool {
    let Some(content_type) = headers
        .get(CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
    else {
        return false;
    };
    let media_type = content_type
        .split(';')
        .next()
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase();
    media_type == "application/json" || media_type.ends_with("+json")
}

fn render_protected(res: &mut Response, scheme: &str, body: Vec<u8>) {
    clear_transformed_headers(res.headers_mut());
    res.headers_mut().insert(
        HeaderName::from_static(DATA_PROTECTION_HEADER),
        HeaderValue::from_static(DATA_PROTECTION_SCHEME),
    );
    res.headers_mut().insert(
        CONTENT_TYPE,
        HeaderValue::from_static(PROTECTED_CONTENT_TYPE),
    );
    res.headers_mut()
        .insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
    append_vary(res.headers_mut());
    debug_assert_eq!(scheme, DATA_PROTECTION_SCHEME);
    res.replace_body(ResBody::Once(body.into()));
}

fn render_failure(
    res: &mut Response,
    status: StatusCode,
    code: &'static str,
    message: &'static str,
) {
    #[derive(Serialize)]
    struct FailureBody {
        protected: bool,
        error: FailureDetail,
    }

    #[derive(Serialize)]
    struct FailureDetail {
        code: &'static str,
        message: &'static str,
    }

    let body = serde_json::to_vec(&FailureBody {
        protected: false,
        error: FailureDetail { code, message },
    })
    .unwrap_or_else(|_| b"{\"protected\":false}".to_vec());

    res.status_code(status);
    res.headers_mut().clear();
    res.headers_mut()
        .insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
    res.headers_mut()
        .insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
    append_vary(res.headers_mut());
    res.replace_body(ResBody::Once(body.into()));
}

fn clear_transformed_headers(headers: &mut HeaderMap) {
    headers.remove(CONTENT_LENGTH);
    headers.remove(CONTENT_ENCODING);
    headers.remove(ETAG);
}

fn append_vary(headers: &mut HeaderMap) {
    let already_varies = headers
        .get_all(VARY)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|value| value.split(','))
        .any(|name| name.trim().eq_ignore_ascii_case(DATA_PROTECTION_HEADER));
    if !already_varies {
        headers.append(VARY, HeaderValue::from_static("X-Data-Protection"));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;

    use aes_gcm::aead::Aead;
    use salvo::http::header::HeaderValue;
    use salvo::test::{ResponseExt, TestClient};
    use serde_json::{Value, json};

    fn test_config() -> DataProtectionConfig {
        DataProtectionConfig {
            key: [0x2a; 32],
            key_id: "test-key".to_owned(),
        }
    }

    fn test_service(middleware: DataProtection) -> Service {
        Service::new(Router::new().hoop(middleware).get(json_response))
    }

    #[handler]
    async fn json_response(res: &mut Response) {
        res.headers_mut()
            .append(VARY, HeaderValue::from_static("Accept-Encoding"));
        res.render(Json(json!({"secret": "业务原文", "count": 2})));
    }

    #[handler]
    async fn text_response() -> &'static str {
        "sensitive plain text"
    }

    fn decrypt(envelope: &Value, config: &DataProtectionConfig) -> Vec<u8> {
        let nonce = URL_SAFE_NO_PAD
            .decode(envelope["data"]["nonce"].as_str().unwrap())
            .unwrap();
        let ciphertext = URL_SAFE_NO_PAD
            .decode(envelope["data"]["ciphertext"].as_str().unwrap())
            .unwrap();
        let cipher = Aes256Gcm::new_from_slice(&config.key).unwrap();
        let compressed = cipher
            .decrypt(
                Nonce::from_slice(&nonce),
                Payload {
                    msg: &ciphertext,
                    aad: AAD,
                },
            )
            .unwrap();
        zstd::stream::decode_all(compressed.as_slice()).unwrap()
    }

    #[test]
    fn standard_base64_key_must_decode_to_exactly_32_bytes() {
        let encoded = STANDARD.encode([7_u8; 32]);
        assert_eq!(decode_key(&encoded).unwrap(), [7_u8; 32]);
        assert_eq!(decode_key("not base64"), Err(ConfigError::InvalidKey));
        assert_eq!(
            decode_key(&STANDARD.encode([7_u8; 31])),
            Err(ConfigError::InvalidKey)
        );
        assert_eq!(
            decode_key(&URL_SAFE_NO_PAD.encode([0xff_u8; 32])),
            Err(ConfigError::InvalidKey)
        );
    }

    #[test]
    fn missing_key_is_optional_but_present_invalid_key_is_rejected() {
        assert!(
            DataProtectionConfig::from_values(None, None)
                .unwrap()
                .is_none()
        );
        assert!(matches!(
            DataProtectionConfig::from_values(Some("invalid"), None),
            Err(ConfigError::InvalidKey)
        ));

        let encoded = STANDARD.encode([7_u8; 32]);
        let config = DataProtectionConfig::from_values(Some(&encoded), Some("  "))
            .unwrap()
            .unwrap();
        assert_eq!(config.key_id, DEFAULT_KEY_ID);
    }

    #[test]
    fn pure_transform_round_trips_exact_original_json_bytes() {
        let config = test_config();
        let original = br#"{ "message": "\u4e1a\u52a1", "items": [1, 2, 3] }"#;
        let nonce = [0x11; NONCE_LENGTH];

        let envelope = protect_json_with_nonce(original, &config, &nonce).unwrap();
        let envelope = serde_json::to_value(envelope).unwrap();

        assert_eq!(envelope["protected"], true);
        assert_eq!(envelope["data"]["version"], 1);
        assert_eq!(envelope["data"]["scheme"], DATA_PROTECTION_SCHEME);
        assert_eq!(envelope["data"]["key_id"], "test-key");
        assert!(!envelope["data"]["nonce"].as_str().unwrap().contains('='));
        assert!(
            !envelope["data"]["ciphertext"]
                .as_str()
                .unwrap()
                .contains('=')
        );
        assert_eq!(decrypt(&envelope, &config), original);
    }

    #[test]
    fn chunks_are_joined_in_order_and_other_body_types_are_rejected() {
        let mut chunks = VecDeque::new();
        chunks.push_back(b"{\"a\":".as_slice().into());
        chunks.push_back(b"1}".as_slice().into());
        assert_eq!(
            collect_body(ResBody::Chunks(chunks)).unwrap(),
            br#"{"a":1}"#
        );
        assert_eq!(
            collect_body(ResBody::None),
            Err(ProtectionError::UnsupportedBody)
        );
    }

    #[tokio::test]
    async fn no_request_header_leaves_json_response_unchanged() {
        let service = test_service(DataProtection::with_config(test_config()));
        let mut response = TestClient::get("http://127.0.0.1/").send(&service).await;

        assert_eq!(response.status_code, Some(StatusCode::OK));
        assert!(response.headers().get(DATA_PROTECTION_HEADER).is_none());
        assert_eq!(
            response.take_json::<Value>().await.unwrap(),
            json!({"secret": "业务原文", "count": 2})
        );
    }

    #[tokio::test]
    async fn requested_json_response_can_be_decrypted_and_decompressed() {
        let config = test_config();
        let service = test_service(DataProtection::with_config(config.clone()));
        let mut response = TestClient::get("http://127.0.0.1/")
            .add_header(DATA_PROTECTION_HEADER, DATA_PROTECTION_SCHEME, true)
            .send(&service)
            .await;

        assert_eq!(response.status_code, Some(StatusCode::OK));
        assert_eq!(
            response
                .headers()
                .get(DATA_PROTECTION_HEADER)
                .unwrap()
                .to_str()
                .unwrap(),
            DATA_PROTECTION_SCHEME
        );
        assert_eq!(
            response.headers().get(CONTENT_TYPE).unwrap(),
            PROTECTED_CONTENT_TYPE
        );
        assert_eq!(response.headers().get(CACHE_CONTROL).unwrap(), "no-store");
        let vary = response
            .headers()
            .get_all(VARY)
            .iter()
            .map(|value| value.to_str().unwrap())
            .collect::<Vec<_>>();
        assert!(vary.contains(&"Accept-Encoding"));
        assert!(vary.contains(&"X-Data-Protection"));

        let envelope = response.take_json::<Value>().await.unwrap();
        let original = decrypt(&envelope, &config);
        assert_eq!(
            serde_json::from_slice::<Value>(&original).unwrap(),
            json!({"secret": "业务原文", "count": 2})
        );
    }

    #[tokio::test]
    async fn unsupported_scheme_fails_before_downstream_without_plaintext() {
        let service = test_service(DataProtection::with_config(test_config()));
        let mut response = TestClient::get("http://127.0.0.1/")
            .add_header(DATA_PROTECTION_HEADER, "unknown", true)
            .send(&service)
            .await;

        assert_eq!(response.status_code, Some(StatusCode::BAD_REQUEST));
        let body = response.take_string().await.unwrap();
        assert!(body.contains("unsupported_data_protection"));
        assert!(!body.contains("业务原文"));
    }

    #[tokio::test]
    async fn repeated_negotiation_header_is_rejected_even_when_values_are_valid() {
        for second_value in [DATA_PROTECTION_SCHEME, "unknown"] {
            let service = test_service(DataProtection::with_config(test_config()));
            let mut response = TestClient::get("http://127.0.0.1/")
                .add_header(DATA_PROTECTION_HEADER, DATA_PROTECTION_SCHEME, false)
                .add_header(DATA_PROTECTION_HEADER, second_value, false)
                .send(&service)
                .await;

            assert_eq!(response.status_code, Some(StatusCode::BAD_REQUEST));
            let body = response.take_string().await.unwrap();
            assert!(body.contains("unsupported_data_protection"));
            assert!(!body.contains("业务原文"));
        }
    }

    #[tokio::test]
    async fn missing_configuration_fails_before_downstream_without_plaintext() {
        let service = test_service(DataProtection::unavailable());
        let mut response = TestClient::get("http://127.0.0.1/")
            .add_header(DATA_PROTECTION_HEADER, DATA_PROTECTION_SCHEME, true)
            .send(&service)
            .await;

        assert_eq!(response.status_code, Some(StatusCode::SERVICE_UNAVAILABLE));
        let body = response.take_string().await.unwrap();
        assert!(body.contains("data_protection_unavailable"));
        assert!(!body.contains("业务原文"));
    }

    #[tokio::test]
    async fn non_json_body_is_discarded_when_protection_was_requested() {
        let service = Service::new(
            Router::new()
                .hoop(DataProtection::with_config(test_config()))
                .get(text_response),
        );
        let mut response = TestClient::get("http://127.0.0.1/")
            .add_header(DATA_PROTECTION_HEADER, DATA_PROTECTION_SCHEME, true)
            .send(&service)
            .await;

        assert_eq!(
            response.status_code,
            Some(StatusCode::INTERNAL_SERVER_ERROR)
        );
        let body = response.take_string().await.unwrap();
        assert!(body.contains("data_protection_failed"));
        assert!(!body.contains("sensitive plain text"));
    }
}
