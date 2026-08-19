use crate::lark::im::{MessageReceiver, ReceiveIdType};
use crate::lark::message_history::CardMessageCategory;
use crate::pipeline::audit_result_sync::AuditResultSyncResult;
use crate::pipeline::project_report::{self, ProjectReportResult};
use crate::server::error::{ApiError, ApiResult, request_id_from_depot};
use crate::server::query::{self, PageQuery};
use crate::server::state::state_from_depot;
use crate::workflow::{
    AuditNoticeRunResult, LoginCheckResult, ManualRegistrationSyncResult, WorkflowRunResult,
    parse_workflow_kind,
};
use crate::xingtu::XingtuSession;
use salvo::extract::{Extractible, Metadata};
use salvo::oapi::extract::JsonBody;
use salvo::oapi::{
    Components, EndpointArgRegister, Operation, ToParameters, ToRequestBody, ToSchema,
};
use salvo::prelude::*;
use salvo::timeout::Timeout;
use serde::{Deserialize, Serialize};

pub mod health {
    use super::*;

    #[endpoint(tags("health"), summary = "健康检查")]
    pub async fn health(depot: &mut Depot) -> ApiResult<HealthResponse> {
        let state = state_from_depot(depot)?;
        sqlx::query("SELECT 1").fetch_one(&state.pool).await?;

        Ok(Json(HealthResponse {
            ok: true,
            status: "healthy".to_string(),
            version: crate::version::version().to_string(),
            git_commit: crate::version::GIT_COMMIT.to_string(),
            built_at: crate::version::BUILD_TIME.to_string(),
        }))
    }

    #[endpoint(tags("health"), summary = "进程存活检查")]
    pub async fn live() -> Json<LiveResponse> {
        Json(LiveResponse {
            ok: true,
            status: "alive".to_owned(),
            version: crate::version::version().to_owned(),
        })
    }

    #[endpoint(tags("health"), summary = "服务就绪检查")]
    pub async fn ready(depot: &mut Depot) -> ApiResult<ReadyResponse> {
        let state = state_from_depot(depot)?;
        sqlx::query("SELECT 1 FROM query_cache_revision WHERE singleton = true")
            .fetch_one(&state.pool)
            .await?;
        Ok(Json(ReadyResponse {
            ok: true,
            status: "ready".to_owned(),
            database: "ready".to_owned(),
            version: crate::version::version().to_owned(),
        }))
    }
}

pub fn routes() -> Router {
    Router::with_path("api/v1")
        .hoop(crate::server::error::request_context)
        .push(crate::server::admin::routes())
        .push(
            Router::with_path("projects/{activity_period_id}/report/send")
                .hoop(crate::server::auth::require_mutation_token)
                .post(send_project_report),
        )
        .push(
            Router::with_path("xingtu")
                .push(
                    Router::with_path("sessions")
                        .hoop(crate::server::auth::require_xingtu_session_upload_token)
                        .post(upsert_xingtu_session),
                )
                .push(
                    Router::with_path("sessions/{account_id}/check")
                        .hoop(crate::server::auth::require_mutation_token)
                        .get(check_xingtu_session),
                )
                .push(
                    Router::with_path("sessions/check-all")
                        .hoop(crate::server::auth::require_mutation_token)
                        .post(check_all_xingtu_sessions),
                ),
        )
        .push(
            Router::with_path("workflows")
                .hoop(crate::server::auth::require_mutation_token)
                .push(Router::with_path("audit/run").post(run_audit_notice))
                .push(Router::with_path("audit-results/sync").post(sync_audit_results))
                .push(Router::with_path("manual-sync/run").post(run_manual_sync))
                .push(Router::with_path("{kind}/run").post(run_workflow))
                .push(Router::with_path("pending/import").post(import_pending_sources)),
        )
        .push(
            Router::with_path("queries")
                .hoop(Timeout::new(std::time::Duration::from_secs(30)))
                .push(Router::with_path("projects").get(list_current_projects))
                .push(Router::with_path("periods").get(list_periods))
                .push(Router::with_path("contents").get(list_contents))
                .push(Router::with_path("feishu-sources").get(list_feishu_sources))
                .push(Router::with_path("pending-summary").get(pending_summary))
                .push(Router::with_path("videos").get(list_videos))
                .push(Router::with_path("videos/with-metrics").get(list_videos_with_metrics))
                .push(Router::with_path("videos/summary").get(video_summary))
                .push(Router::with_path("videos/top-growth").get(top_video_growth))
                .push(Router::with_path("v2/audit-extra/search").post(search_audit_extra))
                .push(Router::with_path("video-metrics").get(list_video_metrics))
                .push(Router::with_path("video-trace-metrics").get(list_video_trace_metrics))
                .push(Router::with_path("live-sessions").get(list_live_sessions))
                .push(Router::with_path("lives/summary").get(live_summary))
                .push(Router::with_path("v2/videos/label-summary").get(video_label_summary))
                .push(Router::with_path("v2/videos").get(list_videos_v2))
                .push(Router::with_path("v2/live-sessions").get(list_live_sessions_v2))
                .push(Router::with_path("v2/feishu-sources").get(list_feishu_sources_v2)),
        )
}

#[endpoint(tags("queries"), summary = "查询全部当前项目")]
async fn list_current_projects(depot: &mut Depot) -> ApiResult<Vec<query::CurrentProjectDto>> {
    let state = state_from_depot(depot)?;
    let cache = state.query_cache.clone();
    let pool = state.pool.clone();
    let data = cache
        .get_or_try_insert("current-projects".to_string(), || async move {
            query::list_current_projects(&pool).await
        })
        .await?;
    Ok(Json(data))
}

#[endpoint(tags("projects"), summary = "发送项目数据汇报卡片")]
async fn send_project_report(
    path: ProjectPath,
    body: RequiredJsonBody<SendProjectReportRequest>,
    depot: &mut Depot,
) -> ApiResult<ProjectReportResponse> {
    if path.activity_period_id <= 0 {
        return Err(ApiError::bad_request("activity_period_id 必须大于 0"));
    }

    let body = body.into_inner();
    let mission = body.mission.trim();
    if mission.is_empty() {
        return Err(ApiError::bad_request("mission 不能为空"));
    }
    let chat_id = body.chat_id.trim();
    if chat_id.is_empty() {
        return Err(ApiError::bad_request("chat_id 不能为空"));
    }

    let state = state_from_depot(depot)?;
    let context = project_report::load_project_report_context(&state.pool, path.activity_period_id)
        .await?
        .ok_or_else(|| {
            ApiError::not_found(format!("当前启用项目不存在：{}", path.activity_period_id))
        })?;
    let result = project_report::send_project_report(
        &state.workflow.lark,
        context,
        mission,
        body.hot_videos.as_deref(),
        chat_id,
    )
    .await?;
    let receiver = MessageReceiver {
        receive_id_type: ReceiveIdType::ChatId,
        receive_id: chat_id.to_string(),
        uuid: None,
    };
    state
        .workflow
        .message_history_repo
        .record_sent_message(
            result.message_id.as_deref(),
            CardMessageCategory::DailyReport,
            format!(
                "{} {} 日报（{}）",
                result.project, result.period, result.date
            ),
            &receiver,
            Some(&result.project),
            Some(result.activity_period_id),
        )
        .await;

    Ok(Json(ProjectReportResponse {
        ok: true,
        data: result,
    }))
}

#[derive(Debug, Serialize, ToSchema)]
pub struct HealthResponse {
    pub ok: bool,
    pub status: String,
    pub version: String,
    pub git_commit: String,
    pub built_at: String,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct LiveResponse {
    pub ok: bool,
    pub status: String,
    pub version: String,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct ReadyResponse {
    pub ok: bool,
    pub status: String,
    pub database: String,
    pub version: String,
}

#[derive(Debug, Deserialize, Serialize, ToSchema)]
struct UpsertSessionRequest {
    /// 项目星图账号 ID。
    xingtu_account_id: String,
    /// xingtu.cn 登录 Cookie。
    cookie: String,
    /// 星图 CSRF Token。
    csrf_token: String,
    /// 可选 session key。
    session_key: Option<String>,
    /// 浏览器 User-Agent。
    user_agent: Option<String>,
    #[serde(default)]
    /// 额外请求头。
    extra_headers: std::collections::HashMap<String, String>,
}

#[derive(Debug, Deserialize, Serialize, ToParameters, ToSchema)]
#[salvo(parameters(default_parameter_in = Path))]
struct AccountPath {
    /// 星图账号 ID。
    account_id: String,
}

#[derive(Debug, Deserialize, Serialize, ToParameters, ToSchema)]
#[salvo(parameters(default_parameter_in = Path))]
struct WorkflowPath {
    /// 工作流类型：morning、periodic、night。
    kind: String,
}

#[derive(Debug, Default, Deserialize, Serialize, ToSchema)]
struct WorkflowScopeRequest {
    /// 可选活动期次内部 ID；不传时按顺序执行全部符合条件的项目。
    activity_period_id: Option<i64>,
}

#[derive(Debug, Deserialize, Serialize, ToParameters, ToSchema)]
#[salvo(parameters(default_parameter_in = Path))]
struct ProjectPath {
    /// 活动期次内部 ID。
    activity_period_id: i64,
}

#[derive(Debug, Deserialize, Serialize, ToSchema)]
struct SendProjectReportRequest {
    /// 今日事项 Markdown。
    mission: String,
    /// 平台热点 Markdown；为空时使用无热点模板。
    hot_videos: Option<String>,
    /// 接收消息的飞书群聊 ID。
    chat_id: String,
}

#[derive(Debug, Serialize, ToSchema)]
struct OkResponse {
    ok: bool,
}

#[derive(Debug, Serialize, ToSchema)]
struct CheckSessionResponse {
    ok: bool,
    data: LoginCheckResult,
}

#[derive(Debug, Serialize, ToSchema)]
struct CheckAllSessionsResponse {
    ok: bool,
    data: Vec<LoginCheckResult>,
}

#[derive(Debug, Serialize, ToSchema)]
struct WorkflowRunResponse {
    ok: bool,
    data: WorkflowRunResult,
}

#[derive(Debug, Serialize, ToSchema)]
struct AuditNoticeRunResponse {
    ok: bool,
    data: AuditNoticeRunResult,
}

#[derive(Debug, Serialize, ToSchema)]
struct AuditResultSyncResponse {
    ok: bool,
    data: AuditResultSyncResult,
}

#[derive(Debug, Serialize, ToSchema)]
struct ManualRegistrationSyncResponse {
    ok: bool,
    data: ManualRegistrationSyncResult,
}

#[derive(Debug, Serialize, ToSchema)]
struct ProjectReportResponse {
    ok: bool,
    data: ProjectReportResult,
}

#[derive(Debug, Deserialize, Serialize, ToSchema)]
struct ImportPendingRequest {
    /// 最多处理多少条 pending/failed 来源。
    limit: Option<i64>,
    /// 可选活动期次内部 ID。
    activity_period_id: Option<i64>,
}

#[derive(Debug, Serialize, ToSchema)]
struct ImportPendingResponse {
    ok: bool,
    data: crate::xingtu::data_import::PendingImportResult,
}

/// 必填 JSON 请求体。
///
/// Salvo 官方 JsonBody 可以生成 OpenAPI schema，但解析失败时不是项目统一错误体；
/// 这里包一层，把解析错误转换为 ApiError。
#[derive(Debug)]
pub(crate) struct RequiredJsonBody<T>(T);

impl<T> RequiredJsonBody<T> {
    pub(crate) fn into_inner(self) -> T {
        self.0
    }
}

impl<'ex, T> Extractible<'ex> for RequiredJsonBody<T>
where
    T: Deserialize<'ex> + Send,
{
    fn metadata() -> &'static Metadata {
        static METADATA: Metadata = Metadata::new("");
        &METADATA
    }

    #[allow(refining_impl_trait)]
    async fn extract(req: &'ex mut Request, _depot: &'ex mut Depot) -> Result<Self, ApiError> {
        req.parse_json::<T>()
            .await
            .map(Self)
            .map_err(|error| ApiError::bad_request(format!("解析 JSON 请求体失败：{error}")))
    }
}

impl<'de, T> EndpointArgRegister for RequiredJsonBody<T>
where
    T: Deserialize<'de> + ToSchema,
{
    fn register(components: &mut Components, operation: &mut Operation, _arg: &str) {
        operation.request_body =
            Some(JsonBody::<T>::to_request_body(components).required(true.into()));
    }
}

/// 可选 JSON 请求体。
///
/// 工作流接口历史上允许不传 body，此包装器用于保留默认范围和 limit 行为，
/// 同时仍然向 OpenAPI 注册 JSON body schema。只有空或纯空白请求体会被视为未传；
/// 非空请求体必须是合法 JSON，避免解析失败时意外退化成“执行全部项目”。
#[derive(Debug)]
struct OptionalJsonBody<T>(Option<T>);

impl<T> OptionalJsonBody<T> {
    fn into_inner(self) -> Option<T> {
        self.0
    }
}

impl<'ex, T> Extractible<'ex> for OptionalJsonBody<T>
where
    T: Deserialize<'ex> + Send,
{
    fn metadata() -> &'static Metadata {
        static METADATA: Metadata = Metadata::new("");
        &METADATA
    }

    #[allow(refining_impl_trait)]
    async fn extract(req: &'ex mut Request, _depot: &'ex mut Depot) -> Result<Self, ApiError> {
        let is_empty = req
            .payload()
            .await
            .map(|payload| payload.iter().all(u8::is_ascii_whitespace))
            .map_err(|error| ApiError::bad_request(format!("读取 JSON 请求体失败：{error}")))?;
        if is_empty {
            return Ok(Self(None));
        }

        req.parse_json::<T>()
            .await
            .map(|value| Self(Some(value)))
            .map_err(|error| ApiError::bad_request(format!("解析 JSON 请求体失败：{error}")))
    }
}

impl<'de, T> EndpointArgRegister for OptionalJsonBody<T>
where
    T: Deserialize<'de> + ToSchema,
{
    fn register(components: &mut Components, operation: &mut Operation, _arg: &str) {
        operation.request_body = Some(JsonBody::<T>::to_request_body(components));
    }
}

#[endpoint(tags("xingtu"), summary = "写入或更新星图登录态")]
async fn upsert_xingtu_session(
    body: RequiredJsonBody<UpsertSessionRequest>,
    depot: &mut Depot,
) -> ApiResult<OkResponse> {
    let state = state_from_depot(depot)?;
    let body = body.into_inner();
    let session = XingtuSession {
        cookie: body.cookie,
        csrf_token: body.csrf_token,
        session_key: body.session_key,
        user_agent: body.user_agent,
        extra_headers: body.extra_headers,
    };

    state
        .workflow
        .upsert_xingtu_session(&body.xingtu_account_id, session)
        .await?;
    state.query_cache.invalidate_all_shared().await?;

    Ok(Json(OkResponse { ok: true }))
}

#[endpoint(tags("xingtu"), summary = "检查指定星图账号登录态")]
async fn check_xingtu_session(
    path: AccountPath,
    depot: &mut Depot,
) -> ApiResult<CheckSessionResponse> {
    let state = state_from_depot(depot)?;
    let accounts = state.workflow.account_repo.list_accounts().await?;
    let account = accounts
        .into_iter()
        .find(|account| account.xingtu_account_id == path.account_id)
        .ok_or_else(|| ApiError::bad_request(format!("未知星图账号：{}", path.account_id)))?;
    let result = state.workflow.check_single_login(&account).await?;

    Ok(Json(CheckSessionResponse {
        ok: true,
        data: result,
    }))
}

#[endpoint(tags("xingtu"), summary = "检查所有启用巡检的星图账号登录态")]
async fn check_all_xingtu_sessions(depot: &mut Depot) -> ApiResult<CheckAllSessionsResponse> {
    let state = state_from_depot(depot)?;
    let result = state.workflow.check_all_logins().await?;

    Ok(Json(CheckAllSessionsResponse {
        ok: true,
        data: result,
    }))
}

#[endpoint(tags("workflows"), summary = "手动执行工作流")]
async fn run_workflow(
    path: WorkflowPath,
    body: OptionalJsonBody<WorkflowScopeRequest>,
    depot: &mut Depot,
) -> ApiResult<WorkflowRunResponse> {
    let state = state_from_depot(depot)?;
    let kind = parse_workflow_kind(&path.kind)?;
    let activity_period_id = workflow_activity_period_id(body.into_inner())?;
    let request_id = request_id_from_depot(depot);
    let result = state
        .workflow
        .run_workflow(kind, activity_period_id, "http", Some(&request_id))
        .await;
    let result = crate::server::cache::invalidate_after_write(&state.query_cache, result).await?;

    Ok(Json(WorkflowRunResponse {
        ok: true,
        data: result,
    }))
}

#[endpoint(tags("workflows"), summary = "只统计并发送审核通知")]
async fn run_audit_notice(
    body: OptionalJsonBody<WorkflowScopeRequest>,
    depot: &mut Depot,
) -> ApiResult<AuditNoticeRunResponse> {
    let state = state_from_depot(depot)?;
    let activity_period_id = workflow_activity_period_id(body.into_inner())?;
    let request_id = request_id_from_depot(depot);
    let result = state
        .workflow
        .run_audit_notice_only(activity_period_id, "http", Some(&request_id))
        .await?;

    Ok(Json(AuditNoticeRunResponse {
        ok: true,
        data: result,
    }))
}

#[endpoint(tags("workflows"), summary = "从审核表同步审核结果到数据库")]
async fn sync_audit_results(
    body: OptionalJsonBody<WorkflowScopeRequest>,
    depot: &mut Depot,
) -> ApiResult<AuditResultSyncResponse> {
    let state = state_from_depot(depot)?;
    let activity_period_id = workflow_activity_period_id(body.into_inner())?;
    let request_id = request_id_from_depot(depot);
    let result = state
        .workflow
        .sync_audit_results_only(activity_period_id, "http", Some(&request_id))
        .await;
    let result = crate::server::cache::invalidate_after_write(&state.query_cache, result).await?;

    Ok(Json(AuditResultSyncResponse {
        ok: true,
        data: result,
    }))
}

#[endpoint(tags("workflows"), summary = "只同步直播和视频手动登记数据")]
async fn run_manual_sync(
    body: OptionalJsonBody<WorkflowScopeRequest>,
    depot: &mut Depot,
) -> ApiResult<ManualRegistrationSyncResponse> {
    let state = state_from_depot(depot)?;
    let activity_period_id = workflow_activity_period_id(body.into_inner())?;
    let request_id = request_id_from_depot(depot);
    let result = state
        .workflow
        .sync_manual_registrations_only(activity_period_id, "http", Some(&request_id))
        .await;
    let result = crate::server::cache::invalidate_after_write(&state.query_cache, result).await?;

    Ok(Json(ManualRegistrationSyncResponse {
        ok: true,
        data: result,
    }))
}

#[endpoint(tags("workflows"), summary = "补偿导入 pending/failed 飞书来源")]
async fn import_pending_sources(
    body: OptionalJsonBody<ImportPendingRequest>,
    depot: &mut Depot,
) -> ApiResult<ImportPendingResponse> {
    let state = state_from_depot(depot)?;
    let body = body.into_inner().unwrap_or(ImportPendingRequest {
        limit: Some(200),
        activity_period_id: None,
    });
    let activity_period_id = validate_activity_period_id(body.activity_period_id)?;
    let request_id = request_id_from_depot(depot);
    let imported = state
        .workflow
        .import_pending_sources(
            body.limit.unwrap_or(200),
            activity_period_id,
            "http",
            Some(&request_id),
        )
        .await;
    let imported =
        crate::server::cache::invalidate_after_write(&state.query_cache, imported).await?;

    Ok(Json(ImportPendingResponse {
        ok: true,
        data: imported,
    }))
}

fn workflow_activity_period_id(
    request: Option<WorkflowScopeRequest>,
) -> Result<Option<i64>, ApiError> {
    validate_activity_period_id(request.and_then(|request| request.activity_period_id))
}

fn validate_activity_period_id(activity_period_id: Option<i64>) -> Result<Option<i64>, ApiError> {
    if activity_period_id.is_some_and(|activity_period_id| activity_period_id <= 0) {
        return Err(ApiError::bad_request(
            "activity_period_id 必须是正整数".to_string(),
        ));
    }
    Ok(activity_period_id)
}

#[endpoint(tags("queries"), summary = "查询启用中的活动期次配置")]
async fn list_periods(depot: &mut Depot) -> ApiResult<Vec<query::ActivityPeriodDto>> {
    let state = state_from_depot(depot)?;
    let cache = state.query_cache.clone();
    let pool = state.pool.clone();
    let data = cache
        .get_or_try_insert("periods".to_string(), || async move {
            query::list_activity_periods(&pool).await
        })
        .await?;
    Ok(Json(data))
}

#[endpoint(tags("queries"), summary = "查询活动直播/视频内容配置")]
async fn list_contents(depot: &mut Depot) -> ApiResult<Vec<query::ContentConfigDto>> {
    let state = state_from_depot(depot)?;
    let cache = state.query_cache.clone();
    let pool = state.pool.clone();
    let data = cache
        .get_or_try_insert("contents".to_string(), || async move {
            query::list_content_configs(&pool).await
        })
        .await?;
    Ok(Json(data))
}

#[endpoint(tags("queries"), summary = "查询星图导出的飞书 Sheet 来源")]
async fn list_feishu_sources(
    page: PageQuery,
    depot: &mut Depot,
) -> ApiResult<Vec<query::FeishuSourceDto>> {
    let state = state_from_depot(depot)?;
    let key = format!("feishu-sources:{}:{}", page.limit(), page.offset());
    let cache = state.query_cache.clone();
    let pool = state.pool.clone();
    let data = cache
        .get_or_try_insert(key, || async move {
            query::list_feishu_sources(&pool, page).await
        })
        .await?;
    Ok(Json(data))
}

#[endpoint(tags("queries"), summary = "查询飞书来源待导入/失败数量")]
async fn pending_summary(depot: &mut Depot) -> ApiResult<query::PendingSummaryDto> {
    let state = state_from_depot(depot)?;
    let cache = state.query_cache.clone();
    let pool = state.pool.clone();
    let data = cache
        .get_or_try_insert("pending-summary".to_string(), || async move {
            query::pending_summary(&pool).await
        })
        .await?;
    Ok(Json(data))
}

#[endpoint(tags("queries"), summary = "查询视频/图文基础内容数据")]
async fn list_videos(page: PageQuery, depot: &mut Depot) -> ApiResult<Vec<query::VideoContentDto>> {
    let state = state_from_depot(depot)?;
    let key = format!("videos:{}:{}", page.limit(), page.offset());
    let cache = state.query_cache.clone();
    let pool = state.pool.clone();
    let data = cache
        .get_or_try_insert(key, || async move {
            query::list_video_contents(&pool, page).await
        })
        .await?;
    Ok(Json(data))
}

#[endpoint(tags("queries"), summary = "查询视频每日追踪指标")]
async fn list_video_metrics(
    page: PageQuery,
    depot: &mut Depot,
) -> ApiResult<Vec<query::VideoMetricDto>> {
    let state = state_from_depot(depot)?;
    let key = format!("video-metrics:{}:{}", page.limit(), page.offset());
    let cache = state.query_cache.clone();
    let pool = state.pool.clone();
    let data = cache
        .get_or_try_insert(key, || async move {
            query::list_video_metrics(&pool, page).await
        })
        .await?;
    Ok(Json(data))
}

#[endpoint(tags("queries"), summary = "查询视频每次追踪快照指标")]
async fn list_video_trace_metrics(
    filters: query::VideoAnalyticsQuery,
    depot: &mut Depot,
) -> ApiResult<Vec<query::VideoTraceMetricDto>> {
    filters
        .validate()
        .map_err(|error| ApiError::bad_request(error.to_string()))?;
    let state = state_from_depot(depot)?;
    let key = cache_key("video-trace-metrics", &filters)?;
    let cache = state.query_cache.clone();
    let pool = state.pool.clone();
    let data = cache
        .get_or_try_insert(key, || async move {
            query::list_video_trace_metrics(&pool, filters).await
        })
        .await?;
    Ok(Json(data))
}

#[endpoint(tags("queries"), summary = "查询直播场次最新数据")]
async fn list_live_sessions(
    page: PageQuery,
    depot: &mut Depot,
) -> ApiResult<Vec<query::LiveSessionDto>> {
    let state = state_from_depot(depot)?;
    let key = format!("live-sessions:{}:{}", page.limit(), page.offset());
    let cache = state.query_cache.clone();
    let pool = state.pool.clone();
    let data = cache
        .get_or_try_insert(key, || async move {
            query::list_live_sessions(&pool, page).await
        })
        .await?;
    Ok(Json(data))
}

#[endpoint(tags("analytics"), summary = "查询视频基础信息和线性每日指标")]
async fn list_videos_with_metrics(
    filters: query::VideoAnalyticsQuery,
    depot: &mut Depot,
) -> ApiResult<Vec<query::VideoWithMetricsDto>> {
    filters
        .validate()
        .map_err(|error| ApiError::bad_request(error.to_string()))?;
    let state = state_from_depot(depot)?;
    let key = cache_key("videos-with-metrics", &filters)?;
    let cache = state.query_cache.clone();
    let pool = state.pool.clone();
    let data = cache
        .get_or_try_insert(key, || async move {
            query::list_videos_with_metrics(&pool, filters).await
        })
        .await?;
    Ok(Json(data))
}

#[endpoint(tags("analytics"), summary = "查询视频播放/互动/稿件/作者汇总")]
async fn video_summary(
    filters: query::VideoAnalyticsQuery,
    depot: &mut Depot,
) -> ApiResult<query::VideoSummaryDto> {
    filters
        .validate()
        .map_err(|error| ApiError::bad_request(error.to_string()))?;
    let state = state_from_depot(depot)?;
    let key = cache_key("video-summary", &filters)?;
    let cache = state.query_cache.clone();
    let pool = state.pool.clone();
    let data = cache
        .get_or_try_insert(
            key,
            || async move { query::video_summary(&pool, filters).await },
        )
        .await?;
    Ok(Json(data))
}

#[endpoint(tags("analytics"), summary = "查询直播观看/ACU/场次/主播汇总")]
async fn live_summary(
    filters: query::LiveAnalyticsQuery,
    depot: &mut Depot,
) -> ApiResult<query::LiveSummaryDto> {
    filters
        .validate()
        .map_err(|error| ApiError::bad_request(error.to_string()))?;
    let state = state_from_depot(depot)?;
    let key = cache_key("live-summary", &filters)?;
    let cache = state.query_cache.clone();
    let pool = state.pool.clone();
    let data = cache
        .get_or_try_insert(
            key,
            || async move { query::live_summary(&pool, filters).await },
        )
        .await?;
    Ok(Json(data))
}

#[endpoint(tags("analytics"), summary = "查询播放量增长最快的视频")]
async fn top_video_growth(
    filters: query::VideoGrowthQuery,
    depot: &mut Depot,
) -> ApiResult<Vec<query::VideoGrowthDto>> {
    filters
        .validate()
        .map_err(|error| ApiError::bad_request(error.to_string()))?;
    let state = state_from_depot(depot)?;
    let key = cache_key("top-video-growth", &filters)?;
    let cache = state.query_cache.clone();
    let pool = state.pool.clone();
    let data = cache
        .get_or_try_insert(key, || async move {
            query::top_video_growth(&pool, filters).await
        })
        .await?;
    Ok(Json(data))
}

#[endpoint(tags("analytics"), summary = "按审核标签汇总视频周报数据")]
async fn video_label_summary(
    filters: query::VideoLabelSummaryQuery,
    depot: &mut Depot,
) -> ApiResult<query::VideoLabelSummaryResponse> {
    filters
        .validate()
        .map_err(|error| ApiError::bad_request(error.to_string()))?;
    let state = state_from_depot(depot)?;
    let key = cache_key("video-label-summary", &filters)?;
    let cache = state.query_cache.clone();
    let pool = state.pool.clone();
    let data = cache
        .get_or_try_insert(key, || async move {
            query::video_label_summary(&pool, filters).await
        })
        .await?;
    Ok(Json(data))
}

#[endpoint(
    tags("audit-extra"),
    summary = "按 audit_extra 精确查询视频、直播及最新汇总",
    description = "只读且幂等的 POST 查询接口。project_id 与 activity_period_id 都必填，服务端会校验期次确实属于该主项目；历史期次也可查询。conditions 是 audit_extra 键值对象，至少一项、最多 20 项，多个条件使用 AND；键名和值均区分大小写，值按 JSON 业务类型精确比较，所以数字 1、字符串 \"1\" 和布尔值 true 不相等，JSON null 也不会匹配缺少该键的记录。飞书历史 `{type,value}` 传输包装会先还原为字符串、数字或布尔业务值再比较；调用方只需传业务值，不要传富文本包装。对象要求完整相等，数组要求元素和顺序完全相等。示例：{\"project_id\":1,\"activity_period_id\":2,\"conditions\":{\"rok_key\":\"ROK\",\"key\":\"<保密值>\"},\"audit_result\":\"审核通过\",\"limit\":100,\"offset\":0}。响应同时返回视频最新 video_daily_metric、直播当前最新行和不受分页影响的汇总；total_play_count 是每个命中视频最新快照的播放量之和，total_live_exposure_pv 只统计 live_exposure_pv。视频和直播分别使用相同的 limit/offset 分页。audit_extra 顶层字段名 key 是机密扩展项：允许作为筛选条件，但绝不会出现在 filters、视频或直播响应中，含该条件的请求也不会写入查询缓存。"
)]
async fn search_audit_extra(
    body: RequiredJsonBody<crate::server::audit_extra_query::AuditExtraSearchRequest>,
    depot: &mut Depot,
) -> ApiResult<crate::server::audit_extra_query::AuditExtraSearchResponse> {
    let filters = body
        .into_inner()
        .normalized()
        .map_err(|error| ApiError::bad_request(error.to_string()))?;
    let state = state_from_depot(depot)?;
    let project_id = filters.project_id;
    let activity_period_id = filters.activity_period_id;
    let data = if filters.contains_sensitive_condition() {
        crate::server::audit_extra_query::search(&state.pool, filters).await?
    } else {
        let key = cache_key("v2-audit-extra-search", &filters)?;
        let cache = state.query_cache.clone();
        let pool = state.pool.clone();
        cache
            .get_or_try_insert(key, || async move {
                crate::server::audit_extra_query::search(&pool, filters).await
            })
            .await?
    }
    .ok_or_else(|| {
        ApiError::not_found(format!(
            "主项目 {project_id} 不存在，或活动期次 {activity_period_id} 不属于该项目"
        ))
    })?;
    Ok(Json(data))
}

#[endpoint(tags("queries"), summary = "统一分页查询视频内容")]
async fn list_videos_v2(
    filters: query::OperationalContentQuery,
    depot: &mut Depot,
) -> ApiResult<query::PagedResponse<query::VideoContentDto>> {
    filters
        .resolved_offset()
        .map_err(|error| ApiError::bad_request(error.to_string()))?;
    filters
        .validate()
        .map_err(|error| ApiError::bad_request(error.to_string()))?;
    let state = state_from_depot(depot)?;
    let key = cache_key("v2-videos", &filters)?;
    let cache = state.query_cache.clone();
    let pool = state.pool.clone();
    let data = cache
        .get_or_try_insert(key, || async move {
            query::list_video_contents_v2(&pool, filters).await
        })
        .await?;
    Ok(Json(data))
}

#[endpoint(tags("queries"), summary = "统一分页查询直播内容")]
async fn list_live_sessions_v2(
    filters: query::OperationalContentQuery,
    depot: &mut Depot,
) -> ApiResult<query::PagedResponse<query::LiveSessionDto>> {
    filters
        .resolved_offset()
        .map_err(|error| ApiError::bad_request(error.to_string()))?;
    filters
        .validate()
        .map_err(|error| ApiError::bad_request(error.to_string()))?;
    let state = state_from_depot(depot)?;
    let key = cache_key("v2-live-sessions", &filters)?;
    let cache = state.query_cache.clone();
    let pool = state.pool.clone();
    let data = cache
        .get_or_try_insert(key, || async move {
            query::list_live_sessions_v2(&pool, filters).await
        })
        .await?;
    Ok(Json(data))
}

#[endpoint(tags("queries"), summary = "统一分页查询飞书来源")]
async fn list_feishu_sources_v2(
    filters: query::OperationalContentQuery,
    depot: &mut Depot,
) -> ApiResult<query::PagedResponse<query::FeishuSourceDto>> {
    filters
        .resolved_offset()
        .map_err(|error| ApiError::bad_request(error.to_string()))?;
    filters
        .validate()
        .map_err(|error| ApiError::bad_request(error.to_string()))?;
    let state = state_from_depot(depot)?;
    let key = cache_key("v2-feishu-sources", &filters)?;
    let cache = state.query_cache.clone();
    let pool = state.pool.clone();
    let data = cache
        .get_or_try_insert(key, || async move {
            query::list_feishu_sources_v2(&pool, filters).await
        })
        .await?;
    Ok(Json(data))
}

fn cache_key<T: Serialize>(prefix: &str, query: &T) -> Result<String, ApiError> {
    let encoded = serde_json::to_string(query).map_err(ApiError::internal)?;
    Ok(format!("{prefix}:{encoded}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use salvo::http::StatusCode;
    use salvo::test::{ResponseExt, TestClient};
    use serde_json::{Value, json};

    #[handler]
    async fn optional_json_probe(body: OptionalJsonBody<WorkflowScopeRequest>) -> ApiResult<Value> {
        Ok(Json(match body.into_inner() {
            Some(request) => json!({
                "ok": true,
                "scope": "single",
                "activity_period_id": request.activity_period_id,
            }),
            None => json!({
                "ok": true,
                "scope": "all",
            }),
        }))
    }

    fn optional_json_test_service() -> Service {
        Service::new(Router::new().post(optional_json_probe))
    }

    #[test]
    fn workflow_scope_accepts_only_positive_activity_period_ids() {
        assert_eq!(validate_activity_period_id(None).unwrap(), None);
        assert_eq!(validate_activity_period_id(Some(2)).unwrap(), Some(2));
        assert!(validate_activity_period_id(Some(0)).is_err());
        assert!(validate_activity_period_id(Some(-1)).is_err());
    }

    #[test]
    fn v2_paged_query_endpoints_are_documented() {
        let openapi =
            salvo::oapi::OpenApi::new("test", "1.0.0").merge_router_with_base(&routes(), "");
        for path in [
            "api/v1/queries/v2/videos",
            "api/v1/queries/v2/videos/label-summary",
            "api/v1/queries/v2/live-sessions",
            "api/v1/queries/v2/feishu-sources",
            "api/v1/queries/v2/audit-extra/search",
        ] {
            assert!(
                openapi.paths.contains_key(path),
                "missing {path}; documented paths: {:?}",
                openapi.paths.keys().collect::<Vec<_>>()
            );
        }

        let document_value = serde_json::to_value(&openapi).unwrap();
        let operation = &document_value["paths"]["api/v1/queries/v2/audit-extra/search"]["post"];
        assert_eq!(operation["requestBody"]["required"], true);
        for status in ["200", "400", "404"] {
            assert!(
                operation["responses"].get(status).is_some(),
                "audit-extra response {status} is undocumented"
            );
        }
        let document = serde_json::to_string(&document_value).unwrap();
        for expected in [
            "多个条件使用 AND",
            "键名和值均区分大小写",
            "total_play_count 是每个命中视频最新快照",
            "total_live_exposure_pv",
            "rok_key",
            "audit_extra 顶层字段名 key 是机密扩展项",
            "调用方只需传业务值，不要传富文本包装",
        ] {
            assert!(
                document.contains(expected),
                "missing OpenAPI text: {expected}"
            );
        }
    }

    #[tokio::test]
    async fn audit_extra_search_rejects_missing_conditions_before_database_access() {
        let service = Service::new(routes());
        let mut response =
            TestClient::post("http://127.0.0.1/api/v1/queries/v2/audit-extra/search")
                .json(&json!({
                    "project_id": 1,
                    "activity_period_id": 2,
                    "conditions": {}
                }))
                .send(&service)
                .await;

        assert_eq!(response.status_code, Some(StatusCode::BAD_REQUEST));
        let body = response.take_json::<Value>().await.unwrap();
        assert_eq!(body["code"], "bad_request");
        assert!(body["message"].as_str().unwrap().contains("conditions"));
    }

    #[tokio::test]
    async fn v2_video_query_rejects_reversed_beijing_date_range() {
        let service = Service::new(routes());
        let mut response = TestClient::get(
            "http://127.0.0.1/api/v1/queries/v2/videos?date_from=2026-08-09&date_to=2026-08-03",
        )
        .send(&service)
        .await;
        assert_eq!(response.status_code, Some(StatusCode::BAD_REQUEST));
        let body = response.take_json::<Value>().await.unwrap();
        assert_eq!(body["code"], "bad_request");
        assert!(body["message"].as_str().unwrap().contains("date_from"));
    }

    #[tokio::test]
    async fn optional_json_body_treats_empty_or_whitespace_body_as_none() {
        let service = optional_json_test_service();
        let empty = TestClient::post("http://127.0.0.1/")
            .send(&service)
            .await
            .take_json::<Value>()
            .await
            .unwrap();
        let whitespace = TestClient::post("http://127.0.0.1/")
            .text(" \n\t")
            .send(&service)
            .await
            .take_json::<Value>()
            .await
            .unwrap();

        assert_eq!(empty["scope"], "all");
        assert_eq!(whitespace["scope"], "all");
    }

    #[tokio::test]
    async fn optional_json_body_parses_valid_json_scope() {
        let service = optional_json_test_service();
        let response = TestClient::post("http://127.0.0.1/")
            .json(&json!({ "activity_period_id": 7 }))
            .send(&service)
            .await
            .take_json::<Value>()
            .await
            .unwrap();

        assert_eq!(response["scope"], "single");
        assert_eq!(response["activity_period_id"], 7);
    }

    #[tokio::test]
    async fn optional_json_body_rejects_malformed_json_instead_of_using_all_scope() {
        let service = optional_json_test_service();
        let mut response = TestClient::post("http://127.0.0.1/")
            .raw_json(r#"{"activity_period_id":7"#)
            .send(&service)
            .await;

        assert_eq!(response.status_code, Some(StatusCode::BAD_REQUEST));
        let body = response.take_json::<Value>().await.unwrap();
        assert_eq!(body["ok"], false);
        assert_ne!(body["scope"], "all");
        assert!(
            body["message"]
                .as_str()
                .is_some_and(|message| message.starts_with("解析 JSON 请求体失败："))
        );
    }

    #[tokio::test]
    async fn optional_json_body_rejects_nonempty_wrong_content_type() {
        let service = optional_json_test_service();
        let mut response = TestClient::post("http://127.0.0.1/")
            .text(r#"{"activity_period_id":7}"#)
            .send(&service)
            .await;

        assert_eq!(response.status_code, Some(StatusCode::BAD_REQUEST));
        let body = response.take_json::<Value>().await.unwrap();
        assert_eq!(body["ok"], false);
        assert!(
            body["message"]
                .as_str()
                .is_some_and(|message| message.contains("Content-Type"))
        );
    }
}
