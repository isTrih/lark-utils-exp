use anyhow::{Context, anyhow};
use reqwest::{
    Client, Method, StatusCode,
    header::{ACCEPT, CONTENT_TYPE, COOKIE, HeaderMap, HeaderName, HeaderValue, USER_AGENT},
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{collections::HashMap, sync::Arc, time::Duration};
use tokio::{
    sync::RwLock,
    time::{Instant, sleep},
};

pub mod account;
pub mod activity_config;
pub mod data_import;
pub mod trace;

const XINGTU_BASE_URL: &str = "https://www.xingtu.cn";
const XINGTU_CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const XINGTU_REQUEST_TIMEOUT: Duration = Duration::from_secs(60);
const XINGTU_REQUEST_ATTEMPTS: u32 = 3;
const DEFAULT_USER_AGENT: &str = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) \
AppleWebKit/537.36 (KHTML, like Gecko) Chrome/148.0.0.0 Safari/537.36";

/// 星图登录态。
///
/// 当前先由调用方注入 cookie/csrf；后续如果做服务接口，
/// 只需要把接口收到的登录态转换成这个结构再调用 `set_session`。
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct XingtuSession {
    pub cookie: String,
    pub csrf_token: String,
    #[serde(default)]
    pub session_key: Option<String>,
    #[serde(default)]
    pub user_agent: Option<String>,
    #[serde(default)]
    pub extra_headers: HashMap<String, String>,
}

impl XingtuSession {
    /// 创建登录态，适合 008 或调度器手动注入。
    pub fn new(cookie: impl Into<String>, csrf_token: impl Into<String>) -> Self {
        Self {
            cookie: cookie.into(),
            csrf_token: csrf_token.into(),
            session_key: None,
            user_agent: None,
            extra_headers: HashMap::new(),
        }
    }

    /// 校验必要字段，避免请求发出去才发现登录态为空。
    fn validate(&self) -> anyhow::Result<()> {
        if self.cookie.trim().is_empty() {
            return Err(anyhow!("星图 cookie 不能为空"));
        }

        if self.csrf_token.trim().is_empty() {
            return Err(anyhow!("星图 csrf_token 不能为空"));
        }

        Ok(())
    }
}

/// 星图任务导出配置。
#[derive(Debug, Clone)]
pub struct ExportTaskOptions {
    pub order_by: String,
    pub poll_interval_millis: u64,
    pub timeout_secs: u64,
}

impl Default for ExportTaskOptions {
    fn default() -> Self {
        Self {
            order_by: "-play".to_string(),
            poll_interval_millis: 2_000,
            timeout_secs: 1_200,
        }
    }
}

/// 星图任务导出结果。
#[derive(Debug, Clone, Serialize)]
pub struct ExportTaskResult {
    pub task_id: String,
    pub ticket_id: String,
    pub status: i64,
    pub spreadsheet_urls: Vec<String>,
    pub logid: Option<String>,
    pub polls: u64,
}

/// 星图客户端。
///
/// 内部保存一份可替换的登录态，方便启动后先注入登录态，再执行拉取。
#[derive(Clone)]
pub struct XingtuClient {
    session: Arc<RwLock<Option<XingtuSession>>>,
    http: Client,
    base_url: Arc<str>,
}

impl XingtuClient {
    /// 创建星图客户端。
    pub fn new() -> anyhow::Result<Self> {
        let http = Client::builder()
            .redirect(reqwest::redirect::Policy::limited(10))
            .connect_timeout(XINGTU_CONNECT_TIMEOUT)
            .timeout(XINGTU_REQUEST_TIMEOUT)
            .build()
            .context("创建星图 HTTP client 失败")?;

        Ok(Self {
            session: Arc::new(RwLock::new(None)),
            http,
            base_url: Arc::from(XINGTU_BASE_URL),
        })
    }

    /// 注入或更新星图登录态。
    ///
    /// 这个方法就是后续“启动时保留一个接口用于接受登录态”的核心入口；
    /// 008 先用环境变量模拟调用，后面可以很自然地接 HTTP/RPC/后端配置。
    pub async fn set_session(&self, session: XingtuSession) -> anyhow::Result<()> {
        session.validate()?;
        *self.session.write().await = Some(session);
        Ok(())
    }

    /// 检查是否已经注入登录态。
    pub async fn has_session(&self) -> bool {
        self.session.read().await.is_some()
    }

    /// 校验当前登录态是否可用。
    pub async fn check_session(&self) -> anyhow::Result<CheckSessionResult> {
        let session = self.require_session().await?;
        let raw = self
            .get_json(&session, "/gw/api/demander/star_demander_basic_info", true)
            .await?;
        let base_resp = parse_base_resp(&raw);
        let valid = base_resp.as_ref().is_some_and(|base| base.status_code == 0);

        Ok(CheckSessionResult {
            valid,
            status_code: base_resp.as_ref().map(|base| base.status_code),
            status_message: base_resp.map(|base| base.status_message),
            raw,
        })
    }

    /// 根据星图任务 ID 导出并返回飞书 spreadsheet URL。
    pub async fn export_task_spreadsheet_urls(
        &self,
        task_id: impl Into<String>,
        options: ExportTaskOptions,
    ) -> anyhow::Result<ExportTaskResult> {
        let task_id = task_id.into();
        let task_id = task_id.trim();

        if task_id.is_empty() {
            return Err(anyhow!("星图任务 ID 不能为空"));
        }

        let session = self.require_session().await?;
        let order_by = urlencoding::encode(&options.order_by);
        let download_path = format!(
            "/gw/api/challenge/download_demander_challenge_item_list?challenge_id={task_id}&order_by={order_by}"
        );
        let download_raw = self.get_json(&session, &download_path, false).await?;
        ensure_base_resp_success(&download_raw)?;
        let download: DownloadResponse = serde_json::from_value(download_raw.clone())
            .with_context(|| format!("星图导出接口返回结构异常：{download_raw}"))?;

        let timeout = Duration::from_secs(options.timeout_secs);
        let interval = Duration::from_millis(options.poll_interval_millis.max(500));
        let started_at = Instant::now();
        let deadline = started_at + timeout;
        let mut polls = 0;
        tracing::info!(
            "星图导出长任务已创建：task_id={} ticket_id={} poll_interval_ms={} timeout_secs={}",
            task_id,
            download.ticket_id,
            interval.as_millis(),
            options.timeout_secs
        );

        loop {
            polls += 1;
            let poll_path = format!(
                "/gw/api/async/get_long_task_result?ticket_id={}&scene=1",
                download.ticket_id
            );
            let raw = self.get_json(&session, &poll_path, true).await?;
            let task: LongTaskResponse = serde_json::from_value(raw.clone())
                .with_context(|| format!("星图长任务返回结构异常：{raw}"))?;
            if polls == 1 || polls % 15 == 0 || task.status != 2 {
                tracing::info!(
                    "星图导出长任务轮询：task_id={} ticket_id={} poll={} status={} elapsed_secs={}",
                    task_id,
                    download.ticket_id,
                    polls,
                    task.status,
                    started_at.elapsed().as_secs()
                );
            }

            if let Some(code) = task.code
                && code != 0
            {
                return Err(anyhow!(
                    "星图长任务失败：code={} msg={}",
                    code,
                    task.msg.unwrap_or_default()
                ));
            }

            if task.status == 3 {
                let result_text = task
                    .result
                    .ok_or_else(|| anyhow!("星图长任务已完成，但没有返回 result"))?;
                let result: LongTaskResult = serde_json::from_str(&result_text)
                    .with_context(|| format!("解析星图长任务嵌套结果失败：{result_text}"))?;

                tracing::info!(
                    "星图导出长任务完成：task_id={} ticket_id={} polls={} elapsed_secs={}",
                    task_id,
                    download.ticket_id,
                    polls,
                    started_at.elapsed().as_secs()
                );
                return Ok(ExportTaskResult {
                    task_id: task_id.to_string(),
                    ticket_id: download.ticket_id,
                    status: task.status,
                    spreadsheet_urls: result.data,
                    logid: result.logid,
                    polls,
                });
            }

            if task.status == 4 {
                return Err(anyhow!(
                    "星图长任务失败：ticket_id={} status=4 reason={}",
                    download.ticket_id,
                    long_task_failure_reason(&task)
                ));
            }

            if Instant::now() >= deadline {
                return Err(anyhow!(
                    "等待星图导出结果超时：last_status={} ticket_id={} elapsed_secs={} timeout_secs={}",
                    task.status,
                    download.ticket_id,
                    started_at.elapsed().as_secs(),
                    options.timeout_secs
                ));
            }

            sleep(interval).await;
        }
    }

    async fn require_session(&self) -> anyhow::Result<XingtuSession> {
        self.session
            .read()
            .await
            .clone()
            .ok_or_else(|| anyhow!("星图登录态未配置，请先调用 set_session"))
    }

    async fn get_json(
        &self,
        session: &XingtuSession,
        path: &str,
        login_source: bool,
    ) -> anyhow::Result<Value> {
        let url = format!("{}{path}", self.base_url);
        let headers = build_headers(session, login_source)?;
        for attempt in 1..=XINGTU_REQUEST_ATTEMPTS {
            let response = self
                .http
                .request(Method::GET, &url)
                .headers(headers.clone())
                .send()
                .await;
            match response {
                Ok(response)
                    if retryable_status(response.status()) && attempt < XINGTU_REQUEST_ATTEMPTS =>
                {
                    let delay = retry_delay(attempt);
                    tracing::warn!(
                        attempt,
                        status = %response.status(),
                        delay_ms = delay.as_millis(),
                        path,
                        "星图瞬态 HTTP 状态，准备有限重试"
                    );
                    sleep(delay).await;
                }
                Ok(response) => {
                    let response = response.error_for_status()?;
                    return Ok(response.json::<Value>().await?);
                }
                Err(error)
                    if retryable_request_error(&error) && attempt < XINGTU_REQUEST_ATTEMPTS =>
                {
                    let delay = retry_delay(attempt);
                    tracing::warn!(
                        attempt,
                        delay_ms = delay.as_millis(),
                        path,
                        error = %error,
                        "星图瞬态请求错误，准备有限重试"
                    );
                    sleep(delay).await;
                }
                Err(error) => return Err(error.into()),
            }
        }
        Err(anyhow!("星图请求重试耗尽：{path}"))
    }
}

fn retryable_status(status: StatusCode) -> bool {
    status == StatusCode::TOO_MANY_REQUESTS
        || matches!(
            status,
            StatusCode::BAD_GATEWAY | StatusCode::SERVICE_UNAVAILABLE | StatusCode::GATEWAY_TIMEOUT
        )
}

fn retryable_request_error(error: &reqwest::Error) -> bool {
    error.is_connect() || error.is_timeout() || error.is_request()
}

fn retry_delay(attempt: u32) -> Duration {
    let mut random = [0_u8; 2];
    let jitter = if getrandom::fill(&mut random).is_ok() {
        u16::from_le_bytes(random) as u64 % 251
    } else {
        0
    };
    Duration::from_millis(250 * 2_u64.pow(attempt.saturating_sub(1)) + jitter)
}

/// 星图登录态校验结果。
#[derive(Debug, Clone, Serialize)]
pub struct CheckSessionResult {
    pub valid: bool,
    pub status_code: Option<i64>,
    pub status_message: Option<String>,
    pub raw: Value,
}

#[derive(Debug, Deserialize)]
struct DownloadResponse {
    ticket_id: String,
}

#[derive(Debug, Deserialize)]
struct LongTaskResponse {
    status: i64,
    #[serde(default)]
    result: Option<String>,
    #[serde(default)]
    msg: Option<String>,
    #[serde(default)]
    code: Option<i64>,
}

#[derive(Debug, Deserialize, Serialize)]
struct LongTaskResult {
    #[serde(default)]
    logid: Option<String>,
    #[serde(default)]
    data: Vec<String>,
}

fn long_task_failure_reason(task: &LongTaskResponse) -> String {
    let Some(result_text) = task.result.as_deref() else {
        return task.msg.clone().unwrap_or_else(|| "未知错误".to_string());
    };

    let Ok(value) = serde_json::from_str::<Value>(result_text) else {
        return result_text.to_string();
    };

    value
        .get("data")
        .and_then(|data| match data {
            Value::String(message) => Some(message.clone()),
            Value::Array(items) => Some(
                items
                    .iter()
                    .filter_map(Value::as_str)
                    .collect::<Vec<_>>()
                    .join(","),
            ),
            _ => None,
        })
        .filter(|message| !message.trim().is_empty())
        .or_else(|| value.get("msg").and_then(Value::as_str).map(str::to_string))
        .or_else(|| {
            value
                .get("message")
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .or_else(|| task.msg.clone())
        .unwrap_or_else(|| result_text.to_string())
}

#[derive(Debug)]
struct BaseResp {
    status_code: i64,
    status_message: String,
}

fn build_headers(session: &XingtuSession, login_source: bool) -> anyhow::Result<HeaderMap> {
    let mut headers = HeaderMap::new();
    headers.insert(
        ACCEPT,
        HeaderValue::from_static("application/json, text/plain, */*"),
    );
    headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
    headers.insert("Agw-Js-Conv", HeaderValue::from_static("str"));
    headers.insert("X-CSRFToken", HeaderValue::from_str(&session.csrf_token)?);
    headers.insert(COOKIE, HeaderValue::from_str(&session.cookie)?);
    headers.insert(
        USER_AGENT,
        HeaderValue::from_str(session.user_agent.as_deref().unwrap_or(DEFAULT_USER_AGENT))?,
    );

    if login_source {
        headers.insert("x-login-source", HeaderValue::from_static("1"));
    }

    if let Some(session_key) = &session.session_key {
        headers.insert("session_key", HeaderValue::from_str(session_key)?);
    }

    for (name, value) in &session.extra_headers {
        let header_name: HeaderName = name.parse()?;
        headers.insert(header_name, HeaderValue::from_str(value)?);
    }

    Ok(headers)
}

fn ensure_base_resp_success(raw: &Value) -> anyhow::Result<()> {
    let Some(base_resp) = raw.get("base_resp") else {
        return Ok(());
    };

    let status_code = base_resp
        .get("status_code")
        .and_then(Value::as_i64)
        .unwrap_or_default();

    if status_code == 0 {
        return Ok(());
    }

    let status_message = base_resp
        .get("status_message")
        .and_then(Value::as_str)
        .unwrap_or_default();

    Err(anyhow!(
        "星图接口失败：status_code={status_code} status_message={status_message}"
    ))
}

fn parse_base_resp(raw: &Value) -> Option<BaseResp> {
    let base_resp = raw.get("base_resp")?;

    Some(BaseResp {
        status_code: base_resp
            .get("status_code")
            .and_then(Value::as_i64)
            .unwrap_or_default(),
        status_message: base_resp
            .get("status_message")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    #[test]
    fn base_resp_success_accepts_missing_base_resp() {
        ensure_base_resp_success(&json!({ "ticket_id": "123" })).unwrap();
    }

    #[test]
    fn base_resp_failure_returns_error() {
        let err = ensure_base_resp_success(&json!({
            "base_resp": {
                "status_code": 1001,
                "status_message": "未登录"
            }
        }))
        .unwrap_err();

        assert!(err.to_string().contains("1001"));
    }

    #[test]
    fn long_task_failure_reason_reads_nested_data_string() {
        let task = LongTaskResponse {
            status: 4,
            result: Some(r#"{"logid":"1","data":"文档创建失败"}"#.to_string()),
            msg: None,
            code: None,
        };

        assert_eq!(long_task_failure_reason(&task), "文档创建失败");
    }

    #[test]
    fn only_transient_http_statuses_are_retried() {
        for status in [
            StatusCode::TOO_MANY_REQUESTS,
            StatusCode::BAD_GATEWAY,
            StatusCode::SERVICE_UNAVAILABLE,
            StatusCode::GATEWAY_TIMEOUT,
        ] {
            assert!(retryable_status(status));
        }
        assert!(!retryable_status(StatusCode::BAD_REQUEST));
        assert!(!retryable_status(StatusCode::UNAUTHORIZED));
        assert!(!retryable_status(StatusCode::INTERNAL_SERVER_ERROR));
    }

    #[tokio::test]
    async fn transient_xingtu_responses_are_retried_with_a_fixed_limit() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            for attempt in 1..=3 {
                let (mut stream, _) = listener.accept().await.unwrap();
                let mut request = [0_u8; 4096];
                let _ = stream.read(&mut request).await.unwrap();
                let (status, body) = if attempt < 3 {
                    ("503 Service Unavailable", r#"{"error":"temporary"}"#)
                } else {
                    (
                        "200 OK",
                        r#"{"base_resp":{"status_code":0,"status_message":"ok"}}"#,
                    )
                };
                let response = format!(
                    "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                stream.write_all(response.as_bytes()).await.unwrap();
            }
        });
        let http = Client::builder()
            .connect_timeout(Duration::from_secs(1))
            .timeout(Duration::from_secs(2))
            .build()
            .unwrap();
        let client = XingtuClient {
            session: Arc::new(RwLock::new(None)),
            http,
            base_url: Arc::from(format!("http://{address}")),
        };
        client
            .set_session(XingtuSession::new("cookie=value", "csrf"))
            .await
            .unwrap();
        assert!(client.check_session().await.unwrap().valid);
        server.await.unwrap();
    }
}
