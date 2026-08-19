use crate::server::state::state_from_depot;
use chrono::{Datelike, Days, Months, NaiveDate, TimeZone, Utc, Weekday};
use chrono_tz::Asia::Shanghai;
use salvo::prelude::*;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use sha1::{Digest, Sha1};
use sqlx::{PgPool, Row};
use std::env;
use url::Url;

const CONFIG_HTML: &str = include_str!("../../web/data-sync-config/index.html");
const CONFIG_CSS: &str = include_str!("../../web/data-sync-config/styles.css");
const CONFIG_JS: &str = include_str!("../../web/data-sync-config/dist/app.js");

const FIELD_DATE: &str = "date";
const FIELD_WEEKDAY: &str = "weekday";
const FIELD_ACTIVITY_DAY: &str = "activity_day";
const FIELD_DAILY_NEW_AUTHORS: &str = "daily_new_active_authors";
const FIELD_CUMULATIVE_AUTHORS: &str = "cumulative_active_authors";
const FIELD_DAILY_NEW_VIDEOS: &str = "daily_new_videos";
const FIELD_CUMULATIVE_VIDEOS: &str = "cumulative_videos";
const FIELD_DAILY_VIDEO_FINAL_PLAY: &str = "daily_video_final_play";
const FIELD_DAILY_PLAY_INCREMENT: &str = "daily_play_increment";
const FIELD_CUMULATIVE_PLAY: &str = "cumulative_play";
const FIELD_DAILY_NEW_LIVE_PV: &str = "daily_new_live_pv";
const FIELD_CUMULATIVE_LIVE_PV: &str = "cumulative_live_pv";
const FIELD_DAILY_NEW_ANCHORS: &str = "daily_new_anchors";
const FIELD_CUMULATIVE_ANCHORS: &str = "cumulative_anchors";
const FIELD_AVG_ACU: &str = "average_acu";

const CODE_CONFIG_ERROR: i32 = 1_254_400;
const CODE_PERMISSION_ERROR: i32 = 1_254_403;
const CODE_INTERNAL_ERROR: i32 = 1_254_500;
const SIGNATURE_MAX_AGE_SECONDS: i64 = 5 * 60;

pub fn routes() -> Router {
    Router::new()
        .push(Router::with_path("meta.json").get(connector_meta))
        .push(Router::with_path("data-sync/config").get(config_html))
        .push(Router::with_path("data-sync/config/").get(config_html))
        .push(Router::with_path("data-sync/config/styles.css").get(config_css))
        .push(Router::with_path("data-sync/config/app.js").get(config_js))
        .push(
            Router::with_path("api/data-sync")
                .push(Router::with_path("table-meta").post(table_meta))
                .push(Router::with_path("records").post(connector_records)),
        )
}

#[handler]
async fn connector_meta(req: &mut Request, res: &mut Response) {
    let config_ui_uri = config_ui_uri(req);
    res.render(Json(json!({
        "schemaVersion": 1,
        "version": crate::version::version(),
        "type": "data_connector",
        "extraData": {
            "disabledPeriodicSync": false,
            "dataSourceConfigUiUri": config_ui_uri,
            "initHeight": 300,
            "initWidth": 520
        },
        "protocol": {
            "type": "http",
            "httpProtocol": {
                "uris": [
                    {
                        "type": "tableMeta",
                        "uri": "/api/data-sync/table-meta"
                    },
                    {
                        "type": "records",
                        "uri": "/api/data-sync/records"
                    }
                ]
            }
        }
    })));
}

#[handler]
async fn config_html(res: &mut Response) {
    res.render(Text::Html(CONFIG_HTML));
}

#[handler]
async fn config_css(res: &mut Response) {
    res.render(Text::Css(CONFIG_CSS));
}

#[handler]
async fn config_js(res: &mut Response) {
    res.render(Text::Js(CONFIG_JS));
}

#[handler]
async fn table_meta(req: &mut Request, depot: &mut Depot, res: &mut Response) {
    let state = match state_from_depot(depot) {
        Ok(state) => state,
        Err(error) => {
            render_problem(res, ConnectorProblem::internal(error));
            return;
        }
    };
    let request = match parse_connector_request(req, &state.pool).await {
        Ok(request) => request,
        Err(problem) => {
            render_problem(res, problem);
            return;
        }
    };
    let activity_period_id = match request.activity_period_id() {
        Ok(activity_period_id) => activity_period_id,
        Err(problem) => {
            render_problem(res, problem);
            return;
        }
    };
    let project = match load_project(&state.pool, activity_period_id).await {
        Ok(Some(project)) => project,
        Ok(None) => {
            render_problem(
                res,
                ConnectorProblem::config(
                    format!("项目不存在：{activity_period_id}"),
                    format!("Project does not exist: {activity_period_id}"),
                ),
            );
            return;
        }
        Err(error) => {
            tracing::error!(
                activity_period_id,
                error = ?error,
                "读取数据同步项目失败"
            );
            render_problem(res, ConnectorProblem::internal(error));
            return;
        }
    };

    render_success(
        res,
        TableMeta {
            table_name: sanitize_table_name(&format!(
                "{} {} 日报",
                project.project, project.period
            )),
            fields: report_fields(),
        },
    );
}

#[handler]
async fn connector_records(req: &mut Request, depot: &mut Depot, res: &mut Response) {
    let state = match state_from_depot(depot) {
        Ok(state) => state,
        Err(error) => {
            render_problem(res, ConnectorProblem::internal(error));
            return;
        }
    };
    let request = match parse_connector_request(req, &state.pool).await {
        Ok(request) => request,
        Err(problem) => {
            render_problem(res, problem);
            return;
        }
    };
    let activity_period_id = match request.activity_period_id() {
        Ok(activity_period_id) => activity_period_id,
        Err(problem) => {
            render_problem(res, problem);
            return;
        }
    };
    let page = match request.page() {
        Ok(page) => page,
        Err(problem) => {
            render_problem(res, problem);
            return;
        }
    };

    let report = match load_daily_report(&state.pool, activity_period_id).await {
        Ok(Some(report)) => report,
        Ok(None) => {
            render_problem(
                res,
                ConnectorProblem::config(
                    format!("项目不存在：{activity_period_id}"),
                    format!("Project does not exist: {activity_period_id}"),
                ),
            );
            return;
        }
        Err(error) => {
            tracing::error!(
                activity_period_id,
                error = ?error,
                "生成飞书数据同步日报失败"
            );
            render_problem(res, ConnectorProblem::internal(error));
            return;
        }
    };

    let end = page
        .offset
        .saturating_add(page.max_page_size)
        .min(report.rows.len());
    let rows = if page.offset < report.rows.len() {
        &report.rows[page.offset..end]
    } else {
        &[]
    };
    let has_more = end < report.rows.len();
    let records = rows
        .iter()
        .map(|row| ConnectorRecord {
            primary_id: format!(
                "period_{}_{}",
                activity_period_id,
                row.date.format("%Y%m%d")
            ),
            data: row.to_connector_data(report.start_date),
        })
        .collect();

    tracing::info!(
        activity_period_id,
        offset = page.offset,
        page_size = rows.len(),
        total_rows = report.rows.len(),
        has_more,
        "返回飞书数据同步日报"
    );
    render_success(
        res,
        RecordsData {
            next_page_token: has_more.then(|| format!("offset_{end}")),
            has_more,
            records,
        },
    );
}

#[derive(Debug, Deserialize)]
struct ConnectorRequest {
    params: String,
}

impl ConnectorRequest {
    fn params_value(&self) -> Result<Value, ConnectorProblem> {
        serde_json::from_str(&self.params).map_err(|error| {
            ConnectorProblem::config(
                format!("同步参数不是有效 JSON：{error}"),
                format!("Sync parameters are not valid JSON: {error}"),
            )
        })
    }

    fn activity_period_id(&self) -> Result<i64, ConnectorProblem> {
        let params = self.params_value()?;
        let raw_config = params.get("datasourceConfig").ok_or_else(|| {
            ConnectorProblem::config(
                "缺少 datasourceConfig，请重新选择项目",
                "Missing datasourceConfig. Please select the project again.",
            )
        })?;
        let config = match raw_config {
            Value::String(value) => serde_json::from_str::<Value>(value).map_err(|error| {
                ConnectorProblem::config(
                    format!("datasourceConfig 不是有效 JSON：{error}"),
                    format!("datasourceConfig is not valid JSON: {error}"),
                )
            })?,
            Value::Object(_) => raw_config.clone(),
            _ => {
                return Err(ConnectorProblem::config(
                    "datasourceConfig 格式错误，请重新选择项目",
                    "Invalid datasourceConfig. Please select the project again.",
                ));
            }
        };
        let value = config
            .get("activity_period_id")
            .or_else(|| config.get("activityPeriodId"))
            .ok_or_else(|| {
                ConnectorProblem::config(
                    "配置中缺少 activity_period_id",
                    "activity_period_id is missing from the configuration.",
                )
            })?;
        let activity_period_id = value
            .as_i64()
            .or_else(|| value.as_str().and_then(|value| value.parse().ok()))
            .filter(|value| *value > 0)
            .ok_or_else(|| {
                ConnectorProblem::config(
                    "activity_period_id 必须是正整数",
                    "activity_period_id must be a positive integer.",
                )
            })?;
        Ok(activity_period_id)
    }

    fn page(&self) -> Result<Page, ConnectorProblem> {
        let params = self.params_value()?;
        let max_page_size = params
            .get("maxPageSize")
            .and_then(Value::as_u64)
            .unwrap_or(100)
            .clamp(1, 1000) as usize;
        let page_token = params
            .get("pageToken")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let offset = parse_page_token(page_token)?;
        Ok(Page {
            offset,
            max_page_size,
        })
    }
}

#[derive(Debug)]
struct Page {
    offset: usize,
    max_page_size: usize,
}

#[derive(Debug, Serialize)]
struct ConnectorResponse<T> {
    code: i32,
    msg: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    data: Option<T>,
}

#[derive(Debug)]
struct ConnectorProblem {
    code: i32,
    zh: String,
    en: String,
}

impl ConnectorProblem {
    fn config(zh: impl Into<String>, en: impl Into<String>) -> Self {
        Self {
            code: CODE_CONFIG_ERROR,
            zh: zh.into(),
            en: en.into(),
        }
    }

    fn permission(zh: impl Into<String>, en: impl Into<String>) -> Self {
        Self {
            code: CODE_PERMISSION_ERROR,
            zh: zh.into(),
            en: en.into(),
        }
    }

    fn internal(error: impl std::fmt::Display) -> Self {
        tracing::error!(error = %error, "飞书数据同步插件内部错误");
        Self {
            code: CODE_INTERNAL_ERROR,
            zh: "数据同步服务暂时不可用，请稍后重试".to_string(),
            en: "The data sync service is temporarily unavailable. Please try again later."
                .to_string(),
        }
    }

    fn message(&self) -> String {
        json!({ "zh": self.zh, "en": self.en }).to_string()
    }
}

#[derive(Debug, Serialize)]
struct TableMeta {
    #[serde(rename = "tableName")]
    table_name: String,
    fields: Vec<ConnectorField>,
}

#[derive(Debug, Serialize)]
struct ConnectorField {
    #[serde(rename = "fieldID")]
    field_id: &'static str,
    #[serde(rename = "fieldName")]
    field_name: &'static str,
    #[serde(rename = "fieldType")]
    field_type: i32,
    #[serde(rename = "isPrimary")]
    is_primary: bool,
    description: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    property: Option<Value>,
}

#[derive(Debug, Serialize)]
struct RecordsData {
    #[serde(rename = "nextPageToken", skip_serializing_if = "Option::is_none")]
    next_page_token: Option<String>,
    #[serde(rename = "hasMore")]
    has_more: bool,
    records: Vec<ConnectorRecord>,
}

#[derive(Debug, Serialize)]
struct ConnectorRecord {
    #[serde(rename = "primaryID")]
    primary_id: String,
    data: Map<String, Value>,
}

#[derive(Debug)]
struct SyncProject {
    activity_period_id: i64,
    project: String,
    period: String,
    task_month: NaiveDate,
}

#[derive(Debug)]
struct DailyReport {
    start_date: NaiveDate,
    rows: Vec<DailyReportRow>,
}

#[derive(Debug)]
struct VideoDay {
    date: NaiveDate,
    daily_new_active_authors: i64,
    cumulative_active_authors: i64,
    daily_new_videos: i64,
    cumulative_videos: i64,
    daily_video_final_play: i64,
    total_play_count: i64,
}

#[derive(Debug)]
struct LiveDay {
    date: NaiveDate,
    daily_new_anchors: i64,
    cumulative_anchors: i64,
    daily_live_pv: i64,
    cumulative_live_pv: i64,
    average_acu: Option<f64>,
}

#[derive(Debug)]
struct DailyReportRow {
    date: NaiveDate,
    daily_new_active_authors: i64,
    cumulative_active_authors: i64,
    daily_new_videos: i64,
    cumulative_videos: i64,
    daily_video_final_play: i64,
    daily_play_increment: i64,
    cumulative_play: i64,
    daily_new_anchors: i64,
    cumulative_anchors: i64,
    daily_live_pv: i64,
    cumulative_live_pv: i64,
    average_acu: Option<f64>,
}

impl DailyReportRow {
    fn to_connector_data(&self, start_date: NaiveDate) -> Map<String, Value> {
        let mut data = Map::new();
        data.insert(FIELD_DATE.to_string(), Value::from(date_millis(self.date)));
        data.insert(
            FIELD_WEEKDAY.to_string(),
            Value::String(weekday_name(self.date.weekday()).to_string()),
        );
        data.insert(
            FIELD_ACTIVITY_DAY.to_string(),
            Value::String(format!(
                "第{}天",
                self.date.signed_duration_since(start_date).num_days() + 1
            )),
        );
        data.insert(
            FIELD_DAILY_NEW_AUTHORS.to_string(),
            Value::from(self.daily_new_active_authors),
        );
        data.insert(
            FIELD_CUMULATIVE_AUTHORS.to_string(),
            Value::from(self.cumulative_active_authors),
        );
        data.insert(
            FIELD_DAILY_NEW_VIDEOS.to_string(),
            Value::from(self.daily_new_videos),
        );
        data.insert(
            FIELD_CUMULATIVE_VIDEOS.to_string(),
            Value::from(self.cumulative_videos),
        );
        data.insert(
            FIELD_DAILY_VIDEO_FINAL_PLAY.to_string(),
            Value::from(self.daily_video_final_play),
        );
        data.insert(
            FIELD_DAILY_PLAY_INCREMENT.to_string(),
            Value::from(self.daily_play_increment),
        );
        data.insert(
            FIELD_CUMULATIVE_PLAY.to_string(),
            Value::from(self.cumulative_play),
        );
        data.insert(
            FIELD_DAILY_NEW_ANCHORS.to_string(),
            Value::from(self.daily_new_anchors),
        );
        data.insert(
            FIELD_CUMULATIVE_ANCHORS.to_string(),
            Value::from(self.cumulative_anchors),
        );
        data.insert(
            FIELD_DAILY_NEW_LIVE_PV.to_string(),
            Value::from(self.daily_live_pv),
        );
        data.insert(
            FIELD_CUMULATIVE_LIVE_PV.to_string(),
            Value::from(self.cumulative_live_pv),
        );
        if let Some(average_acu) = self.average_acu {
            data.insert(FIELD_AVG_ACU.to_string(), Value::from(average_acu));
        }
        data
    }
}

async fn parse_connector_request(
    req: &mut Request,
    pool: &PgPool,
) -> Result<ConnectorRequest, ConnectorProblem> {
    let signature = SignatureHeaders::from_request(req);
    let body = req
        .payload_with_max_size(64 * 1024)
        .await
        .map_err(|error| {
            ConnectorProblem::config(
                format!("读取请求体失败：{error}"),
                format!("Failed to read request body: {error}"),
            )
        })?
        .clone();
    let body_text = std::str::from_utf8(&body).map_err(|error| {
        ConnectorProblem::config(
            format!("请求体不是 UTF-8：{error}"),
            format!("Request body is not UTF-8: {error}"),
        )
    })?;
    verify_signature(&signature, body_text, pool).await?;
    serde_json::from_slice(&body).map_err(|error| {
        ConnectorProblem::config(
            format!("请求体不是有效 JSON：{error}"),
            format!("Request body is not valid JSON: {error}"),
        )
    })
}

#[derive(Debug, Default)]
struct SignatureHeaders {
    timestamp: Option<String>,
    nonce: Option<String>,
    signature: Option<String>,
}

impl SignatureHeaders {
    fn from_request(req: &Request) -> Self {
        Self {
            timestamp: header_text(req, "x-base-request-timestamp"),
            nonce: header_text(req, "x-base-request-nonce"),
            signature: header_text(req, "x-base-signature"),
        }
    }
}

fn header_text(req: &Request, name: &str) -> Option<String> {
    req.headers()
        .get(name)
        .and_then(|value| value.to_str().ok())
        .map(ToString::to_string)
}

async fn verify_signature(
    headers: &SignatureHeaders,
    body: &str,
    pool: &PgPool,
) -> Result<(), ConnectorProblem> {
    let secret = env::var("DATA_SYNC_SECRET_KEY")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty());
    let Some(secret) = secret else {
        return Ok(());
    };
    let timestamp = headers.timestamp.as_deref().ok_or_else(|| {
        ConnectorProblem::permission(
            "缺少 X-Base-Request-Timestamp",
            "Missing X-Base-Request-Timestamp.",
        )
    })?;
    let nonce = headers.nonce.as_deref().ok_or_else(|| {
        ConnectorProblem::permission("缺少 X-Base-Request-Nonce", "Missing X-Base-Request-Nonce.")
    })?;
    let signature = headers.signature.as_deref().ok_or_else(|| {
        ConnectorProblem::permission("缺少 X-Base-Signature", "Missing X-Base-Signature.")
    })?;
    let expected = request_signature(timestamp, nonce, &secret, body);
    if !constant_time_ascii_case_equal(signature, &expected) {
        return Err(ConnectorProblem::permission(
            "请求签名无效",
            "Invalid request signature.",
        ));
    }

    let request_timestamp = validate_request_timestamp(timestamp)?;
    if nonce.trim().is_empty() || nonce.len() > 256 {
        return Err(ConnectorProblem::permission(
            "请求 nonce 无效",
            "Invalid request nonce.",
        ));
    }
    sqlx::query("DELETE FROM data_sync_request_nonce WHERE expires_at <= now()")
        .execute(pool)
        .await
        .map_err(ConnectorProblem::internal)?;
    let inserted = sqlx::query(
        r#"
        INSERT INTO data_sync_request_nonce (nonce, request_timestamp, expires_at)
        VALUES ($1, to_timestamp($2), now() + interval '10 minutes')
        ON CONFLICT (nonce) DO NOTHING
        "#,
    )
    .bind(nonce)
    .bind(request_timestamp)
    .execute(pool)
    .await
    .map_err(ConnectorProblem::internal)?;
    if inserted.rows_affected() == 0 {
        return Err(ConnectorProblem::permission(
            "请求 nonce 已使用，拒绝重放",
            "Request nonce has already been used.",
        ));
    }
    Ok(())
}

fn validate_request_timestamp(value: &str) -> Result<i64, ConnectorProblem> {
    let timestamp = value.parse::<i64>().map_err(|_| {
        ConnectorProblem::permission("请求 timestamp 无效", "Invalid request timestamp.")
    })?;
    if (Utc::now().timestamp() - timestamp).abs() > SIGNATURE_MAX_AGE_SECONDS {
        return Err(ConnectorProblem::permission(
            "请求 timestamp 已过期或超出允许时钟偏差",
            "Request timestamp is expired or outside the allowed clock skew.",
        ));
    }
    Ok(timestamp)
}

fn constant_time_ascii_case_equal(left: &str, right: &str) -> bool {
    if left.len() != right.len() {
        return false;
    }
    left.bytes()
        .zip(right.bytes())
        .fold(0_u8, |difference, (left, right)| {
            difference | (left.to_ascii_lowercase() ^ right.to_ascii_lowercase())
        })
        == 0
}

fn request_signature(timestamp: &str, nonce: &str, secret: &str, body: &str) -> String {
    let mut sha1 = Sha1::new();
    sha1.update(timestamp.as_bytes());
    sha1.update(nonce.as_bytes());
    sha1.update(secret.as_bytes());
    sha1.update(body.as_bytes());
    format!("{:x}", sha1.finalize())
}

fn config_ui_uri(req: &Request) -> String {
    if let Some(uri) = env_value("DATA_SYNC_CONFIG_UI_URL").and_then(|uri| https_url(&uri)) {
        return uri;
    }
    if let Some(origin) = env_value("DATA_SYNC_PUBLIC_BASE_URL")
        .as_deref()
        .and_then(public_base_origin)
    {
        return format!("{origin}/data-sync/config");
    }

    format!("{}/data-sync/config", request_origin(req))
}

fn request_origin(req: &Request) -> String {
    let forwarded = header_text(req, "forwarded");
    let host = forwarded
        .as_deref()
        .and_then(|value| forwarded_parameter(value, "host"))
        .or_else(|| {
            header_text(req, "x-forwarded-host")
                .as_deref()
                .and_then(first_forwarded_value)
        })
        .or_else(|| header_text(req, "host"))
        .unwrap_or_else(|| "localhost:8080".to_string());

    public_base_origin(&format!("https://{host}"))
        .unwrap_or_else(|| "https://localhost:8080".to_string())
}

fn forwarded_parameter(value: &str, name: &str) -> Option<String> {
    value
        .split(',')
        .next()?
        .split(';')
        .filter_map(|part| part.trim().split_once('='))
        .find(|(key, _)| key.trim().eq_ignore_ascii_case(name))
        .and_then(|(_, value)| clean_forwarded_value(value))
}

fn first_forwarded_value(value: &str) -> Option<String> {
    value.split(',').next().and_then(clean_forwarded_value)
}

fn clean_forwarded_value(value: &str) -> Option<String> {
    let value = value.trim().trim_matches('"').trim();
    (!value.is_empty()).then(|| value.to_ascii_lowercase())
}

fn public_base_origin(base_url: &str) -> Option<String> {
    let url = https_url(base_url)?;
    let url = Url::parse(&url).ok()?;
    let origin = url.origin().ascii_serialization();
    (origin != "null").then_some(origin)
}

fn https_url(value: &str) -> Option<String> {
    let mut url = Url::parse(value).ok()?;
    if !matches!(url.scheme(), "http" | "https") {
        return None;
    }
    url.set_scheme("https").ok()?;
    Some(url.to_string())
}

fn env_value(name: &str) -> Option<String> {
    env::var(name)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn parse_page_token(page_token: &str) -> Result<usize, ConnectorProblem> {
    if page_token.is_empty() {
        return Ok(0);
    }
    page_token
        .strip_prefix("offset_")
        .and_then(|value| value.parse::<usize>().ok())
        .ok_or_else(|| ConnectorProblem::config("pageToken 格式错误", "Invalid pageToken."))
}

fn render_success<T: Serialize + Send>(res: &mut Response, data: T) {
    res.render(Json(ConnectorResponse {
        code: 0,
        msg: String::new(),
        data: Some(data),
    }));
}

fn render_problem(res: &mut Response, problem: ConnectorProblem) {
    res.render(Json(ConnectorResponse::<Value> {
        code: problem.code,
        msg: problem.message(),
        data: None,
    }));
}

async fn load_project(
    pool: &PgPool,
    activity_period_id: i64,
) -> anyhow::Result<Option<SyncProject>> {
    let row = sqlx::query(
        r#"
        SELECT period.activity_period_id, project.display_name AS project,
            period.period, period.task_month
        FROM xingtu_activity_period period
        JOIN xingtu_project project ON project.project_id = period.project_id
        WHERE period.activity_period_id = $1
        "#,
    )
    .bind(activity_period_id)
    .fetch_optional(pool)
    .await?;
    row.map(|row| {
        Ok(SyncProject {
            activity_period_id: row.try_get("activity_period_id")?,
            project: row.try_get("project")?,
            period: row.try_get("period")?,
            task_month: row.try_get("task_month")?,
        })
    })
    .transpose()
}

async fn load_daily_report(
    pool: &PgPool,
    activity_period_id: i64,
) -> anyhow::Result<Option<DailyReport>> {
    let Some(project) = load_project(pool, activity_period_id).await? else {
        return Ok(None);
    };
    let start_date = project.task_month;
    let end_date = start_date
        .checked_add_months(Months::new(1))
        .and_then(|date| date.checked_sub_days(Days::new(1)))
        .ok_or_else(|| anyhow::anyhow!("项目月份超出可计算日期范围"))?;
    let video_days =
        load_video_days(pool, project.activity_period_id, start_date, end_date).await?;
    let live_days = load_live_days(pool, project.activity_period_id, start_date, end_date).await?;
    if video_days.len() != live_days.len() + 1 {
        anyhow::bail!(
            "日报日期数量不一致：video_days={} live_days={}",
            video_days.len(),
            live_days.len()
        );
    }

    let mut rows = Vec::with_capacity(video_days.len());
    let mut previous_total_play = video_days
        .first()
        .map(|day| day.total_play_count)
        .unwrap_or(0);
    for (video, live) in video_days.into_iter().skip(1).zip(live_days) {
        if video.date != live.date {
            anyhow::bail!(
                "日报日期未对齐：video_date={} live_date={}",
                video.date,
                live.date
            );
        }
        let daily_play_increment = video.total_play_count - previous_total_play;
        previous_total_play = video.total_play_count;
        rows.push(DailyReportRow {
            date: video.date,
            daily_new_active_authors: video.daily_new_active_authors,
            cumulative_active_authors: video.cumulative_active_authors,
            daily_new_videos: video.daily_new_videos,
            cumulative_videos: video.cumulative_videos,
            daily_video_final_play: video.daily_video_final_play,
            daily_play_increment,
            cumulative_play: video.total_play_count,
            daily_new_anchors: live.daily_new_anchors,
            cumulative_anchors: live.cumulative_anchors,
            daily_live_pv: live.daily_live_pv,
            cumulative_live_pv: live.cumulative_live_pv,
            average_acu: live.average_acu.map(round_two_decimals),
        });
    }
    Ok(Some(DailyReport { start_date, rows }))
}

async fn load_video_days(
    pool: &PgPool,
    activity_period_id: i64,
    start_date: NaiveDate,
    end_date: NaiveDate,
) -> anyhow::Result<Vec<VideoDay>> {
    let rows = sqlx::query(
        r#"
        WITH video_configs AS (
            SELECT content_config_id
            FROM xingtu_activity_content_config
            WHERE activity_period_id = $1
              AND content_type = 'video'
        ),
        videos AS (
            SELECT
                vc.content_config_id,
                vc.video_id,
                vc.publish_time::date AS publish_date,
                COALESCE(
                    NULLIF(btrim(vc.author_uid), ''),
                    NULLIF(btrim(vc.author_name), '')
                ) AS author_key
            FROM video_content vc
            JOIN video_configs cfg
              ON cfg.content_config_id = vc.content_config_id
        ),
        author_first_date AS (
            SELECT author_key, MIN(publish_date) AS first_date
            FROM videos
            WHERE author_key IS NOT NULL
            GROUP BY author_key
        ),
        report_dates AS (
            SELECT day_value::date AS report_date
            FROM generate_series(
                ($2::date - 1)::timestamp,
                $3::date::timestamp,
                interval '1 day'
            ) AS day_value
        )
        SELECT
            d.report_date,
            (
                SELECT COUNT(*)::bigint
                FROM author_first_date a
                WHERE a.first_date = d.report_date
            ) AS daily_new_active_authors,
            (
                SELECT COUNT(*)::bigint
                FROM author_first_date a
                WHERE a.first_date <= d.report_date
            ) AS cumulative_active_authors,
            (
                SELECT COUNT(*)::bigint
                FROM videos v
                WHERE v.publish_date = d.report_date
            ) AS daily_new_videos,
            (
                SELECT COUNT(*)::bigint
                FROM videos v
                WHERE v.publish_date <= d.report_date
            ) AS cumulative_videos,
            COALESCE((
                SELECT SUM(COALESCE((
                    SELECT m.play_count
                    FROM video_daily_metric m
                    WHERE m.content_config_id = v.content_config_id
                      AND m.video_id = v.video_id
                    ORDER BY m.stat_date DESC
                    LIMIT 1
                ), 0))::bigint
                FROM videos v
                WHERE v.publish_date = d.report_date
            ), 0)::bigint AS daily_video_final_play,
            COALESCE((
                SELECT SUM(COALESCE((
                    SELECT m.play_count
                    FROM video_daily_metric m
                    WHERE m.content_config_id = v.content_config_id
                      AND m.video_id = v.video_id
                      AND m.stat_date <= d.report_date
                    ORDER BY m.stat_date DESC
                    LIMIT 1
                ), 0))::bigint
                FROM videos v
                WHERE v.publish_date <= d.report_date
            ), 0)::bigint AS total_play_count
        FROM report_dates d
        ORDER BY d.report_date
        "#,
    )
    .bind(activity_period_id)
    .bind(start_date)
    .bind(end_date)
    .fetch_all(pool)
    .await?;

    rows.into_iter()
        .map(|row| {
            Ok(VideoDay {
                date: row.try_get("report_date")?,
                daily_new_active_authors: row.try_get("daily_new_active_authors")?,
                cumulative_active_authors: row.try_get("cumulative_active_authors")?,
                daily_new_videos: row.try_get("daily_new_videos")?,
                cumulative_videos: row.try_get("cumulative_videos")?,
                daily_video_final_play: row.try_get("daily_video_final_play")?,
                total_play_count: row.try_get("total_play_count")?,
            })
        })
        .collect()
}

async fn load_live_days(
    pool: &PgPool,
    activity_period_id: i64,
    start_date: NaiveDate,
    end_date: NaiveDate,
) -> anyhow::Result<Vec<LiveDay>> {
    let rows = sqlx::query(
        r#"
        WITH live_configs AS (
            SELECT content_config_id
            FROM xingtu_activity_content_config
            WHERE activity_period_id = $1
              AND content_type = 'live'
        ),
        sessions AS (
            SELECT
                ls.start_time::date AS start_date,
                COALESCE(
                    NULLIF(btrim(ls.anchor_uid), ''),
                    NULLIF(btrim(ls.anchor_name), '')
                ) AS anchor_key,
                ls.live_exposure_pv,
                ls.acu
            FROM live_session ls
            JOIN live_configs cfg
              ON cfg.content_config_id = ls.content_config_id
        ),
        anchor_first_date AS (
            SELECT anchor_key, MIN(start_date) AS first_date
            FROM sessions
            WHERE anchor_key IS NOT NULL
            GROUP BY anchor_key
        ),
        report_dates AS (
            SELECT day_value::date AS report_date
            FROM generate_series(
                $2::date::timestamp,
                $3::date::timestamp,
                interval '1 day'
            ) AS day_value
        )
        SELECT
            d.report_date,
            (
                SELECT COUNT(*)::bigint
                FROM anchor_first_date a
                WHERE a.first_date = d.report_date
            ) AS daily_new_anchors,
            (
                SELECT COUNT(*)::bigint
                FROM anchor_first_date a
                WHERE a.first_date <= d.report_date
            ) AS cumulative_anchors,
            COALESCE((
                SELECT SUM(COALESCE(s.live_exposure_pv, 0))::bigint
                FROM sessions s
                WHERE s.start_date = d.report_date
            ), 0)::bigint AS daily_live_pv,
            COALESCE((
                SELECT SUM(COALESCE(s.live_exposure_pv, 0))::bigint
                FROM sessions s
                WHERE s.start_date <= d.report_date
            ), 0)::bigint AS cumulative_live_pv,
            (
                SELECT AVG(s.acu)::float8
                FROM sessions s
                WHERE s.start_date = d.report_date
            ) AS average_acu
        FROM report_dates d
        ORDER BY d.report_date
        "#,
    )
    .bind(activity_period_id)
    .bind(start_date)
    .bind(end_date)
    .fetch_all(pool)
    .await?;

    rows.into_iter()
        .map(|row| {
            Ok(LiveDay {
                date: row.try_get("report_date")?,
                daily_new_anchors: row.try_get("daily_new_anchors")?,
                cumulative_anchors: row.try_get("cumulative_anchors")?,
                daily_live_pv: row.try_get("daily_live_pv")?,
                cumulative_live_pv: row.try_get("cumulative_live_pv")?,
                average_acu: row.try_get("average_acu")?,
            })
        })
        .collect()
}

fn report_fields() -> Vec<ConnectorField> {
    vec![
        field(
            FIELD_DATE,
            "日期",
            5,
            true,
            "日报日期，按项目 task_month 所在自然月生成",
            Some(json!({ "formatter": "yyyy/MM/dd" })),
        ),
        field(FIELD_WEEKDAY, "星期", 1, false, "日期对应的星期", None),
        field(
            FIELD_ACTIVITY_DAY,
            "活动周期",
            1,
            false,
            "从项目 task_month 起算的活动天数",
            None,
        ),
        number_field(
            FIELD_DAILY_NEW_AUTHORS,
            "每日新增活跃作者",
            "首次发布稿件日期为当天的去重作者数",
            false,
        ),
        number_field(
            FIELD_CUMULATIVE_AUTHORS,
            "累计活跃作者",
            "截至当天已发布稿件的去重作者数",
            false,
        ),
        number_field(
            FIELD_DAILY_NEW_VIDEOS,
            "每日新增视频",
            "发布日期为当天的稿件数",
            false,
        ),
        number_field(
            FIELD_CUMULATIVE_VIDEOS,
            "累计视频",
            "截至当天已发布的稿件数",
            false,
        ),
        number_field(
            FIELD_DAILY_VIDEO_FINAL_PLAY,
            "每日视频最终播放",
            "当天发布稿件的当前最新播放量总和；后续同步会随最终数据更新",
            false,
        ),
        number_field(
            FIELD_DAILY_PLAY_INCREMENT,
            "每日新增播放量",
            "当天所有视频播放总量减去昨日所有视频播放总量",
            false,
        ),
        number_field(
            FIELD_CUMULATIVE_PLAY,
            "累计播放量",
            "截至当天所有视频最新快照的播放总量",
            false,
        ),
        number_field(
            FIELD_DAILY_NEW_ANCHORS,
            "每日新增主播",
            "首次开播日期为当天的去重主播数",
            false,
        ),
        number_field(
            FIELD_CUMULATIVE_ANCHORS,
            "累计主播数",
            "截至当天已开播的去重主播数",
            false,
        ),
        number_field(
            FIELD_DAILY_NEW_LIVE_PV,
            "每日新增观看人次",
            "当天直播场次的 live_exposure_pv 总和",
            false,
        ),
        number_field(
            FIELD_CUMULATIVE_LIVE_PV,
            "累计观看人次",
            "截至当天直播场次的 live_exposure_pv 总和",
            false,
        ),
        field(
            FIELD_AVG_ACU,
            "平均ACU",
            2,
            false,
            "当天直播场次 ACU 的算术平均值",
            Some(json!({ "formatter": "#,##0.00" })),
        ),
    ]
}

fn field(
    field_id: &'static str,
    field_name: &'static str,
    field_type: i32,
    is_primary: bool,
    description: &'static str,
    property: Option<Value>,
) -> ConnectorField {
    ConnectorField {
        field_id,
        field_name,
        field_type,
        is_primary,
        description,
        property,
    }
}

fn number_field(
    field_id: &'static str,
    field_name: &'static str,
    description: &'static str,
    is_primary: bool,
) -> ConnectorField {
    field(
        field_id,
        field_name,
        2,
        is_primary,
        description,
        Some(json!({ "formatter": "#,##0" })),
    )
}

fn sanitize_table_name(name: &str) -> String {
    let sanitized = name
        .chars()
        .filter(|character| !matches!(character, '/' | '\\' | '?' | '*' | '[' | ']' | ':'))
        .take(100)
        .collect::<String>()
        .trim()
        .to_string();
    if sanitized.is_empty() {
        "项目日报".to_string()
    } else {
        sanitized
    }
}

fn weekday_name(weekday: Weekday) -> &'static str {
    match weekday {
        Weekday::Mon => "星期一",
        Weekday::Tue => "星期二",
        Weekday::Wed => "星期三",
        Weekday::Thu => "星期四",
        Weekday::Fri => "星期五",
        Weekday::Sat => "星期六",
        Weekday::Sun => "星期日",
    }
}

fn date_millis(date: NaiveDate) -> i64 {
    Shanghai
        .from_local_datetime(&date.and_hms_opt(0, 0, 0).expect("valid midnight"))
        .single()
        .expect("Asia/Shanghai has an unambiguous midnight")
        .timestamp_millis()
}

fn round_two_decimals(value: f64) -> f64 {
    (value * 100.0).round() / 100.0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_saved_project_config() {
        let request = ConnectorRequest {
            params: json!({
                "datasourceConfig": json!({ "activity_period_id": 7 }).to_string(),
                "pageToken": "",
                "maxPageSize": 20
            })
            .to_string(),
        };

        assert_eq!(request.activity_period_id().unwrap(), 7);
        let page = request.page().unwrap();
        assert_eq!(page.offset, 0);
        assert_eq!(page.max_page_size, 20);
    }

    #[test]
    fn parses_page_offset() {
        assert_eq!(parse_page_token("").unwrap(), 0);
        assert_eq!(parse_page_token("offset_31").unwrap(), 31);
        assert!(parse_page_token("31").is_err());
    }

    #[test]
    fn normalizes_public_base_url_to_origin() {
        assert_eq!(
            public_base_origin("https://sync.example.com/legacy-prefix/"),
            Some("https://sync.example.com".to_string())
        );
        assert_eq!(
            public_base_origin("http://127.0.0.1:8080/path"),
            Some("https://127.0.0.1:8080".to_string())
        );
        assert_eq!(public_base_origin("ftp://sync.example.com"), None);
    }

    #[test]
    fn parses_https_reverse_proxy_headers() {
        let forwarded = "for=192.0.2.60;proto=http;host=api.example.com";

        assert_eq!(
            forwarded_parameter(forwarded, "host").as_deref(),
            Some("api.example.com")
        );
        assert_eq!(
            first_forwarded_value("api.example.com, internal:8080").as_deref(),
            Some("api.example.com")
        );
        assert_eq!(
            public_base_origin("http://api.example.com").as_deref(),
            Some("https://api.example.com")
        );
    }

    #[test]
    fn exposes_csv_columns_in_order() {
        let names = report_fields()
            .into_iter()
            .map(|field| field.field_name)
            .collect::<Vec<_>>();
        assert_eq!(
            names,
            vec![
                "日期",
                "星期",
                "活动周期",
                "每日新增活跃作者",
                "累计活跃作者",
                "每日新增视频",
                "累计视频",
                "每日视频最终播放",
                "每日新增播放量",
                "累计播放量",
                "每日新增主播",
                "累计主播数",
                "每日新增观看人次",
                "累计观看人次",
                "平均ACU",
            ]
        );
    }

    #[test]
    fn uses_beijing_midnight_for_date_values() {
        let date = NaiveDate::from_ymd_opt(2026, 7, 1).unwrap();
        assert_eq!(date_millis(date), 1_782_835_200_000);
        assert_eq!(weekday_name(date.weekday()), "星期三");
    }

    #[test]
    fn calculates_signature_in_documented_order() {
        assert_eq!(
            request_signature(
                "1710000000",
                "nonce",
                "testBase",
                r#"{"params":"{}","context":"{}"}"#
            ),
            "beff25d9372f00ad20f22fed4adbd70477b5282e"
        );
    }

    #[test]
    fn signature_timestamp_rejects_expired_and_future_requests() {
        let now = Utc::now().timestamp();
        assert_eq!(validate_request_timestamp(&now.to_string()).unwrap(), now);
        assert!(
            validate_request_timestamp(&(now - SIGNATURE_MAX_AGE_SECONDS - 1).to_string()).is_err()
        );
        assert!(
            validate_request_timestamp(&(now + SIGNATURE_MAX_AGE_SECONDS + 1).to_string()).is_err()
        );
        assert!(validate_request_timestamp("not-a-number").is_err());
    }
}
