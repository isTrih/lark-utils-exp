use crate::lark::im::{FeishuImClient, parse_receive_id_type};
use crate::lark::message_history::{
    CardMessageCategory, CardMessageHistory, CardMessageHistoryFilter, CardMessageHistoryRepository,
};
use crate::server::api::RequiredJsonBody;
use crate::server::auth::{configured_xingtu_session_upload_token, require_mutation_token};
use crate::server::error::{ApiError, ApiResult};
use crate::server::state::state_from_depot;
use crate::workflow_run::{WorkflowRunRecord, WorkflowStepRecord};
use crate::xingtu::activity_config::{
    ActivityContentConfig, WorkflowConfig, validate_activity_contents,
};
use chrono::{DateTime, Datelike, NaiveDate, Utc};
use salvo::http::header::{CACHE_CONTROL, HeaderValue, PRAGMA};
use salvo::oapi::{ToParameters, ToSchema};
use salvo::prelude::*;
use serde::{Deserialize, Serialize};
use sqlx::postgres::PgRow;
use sqlx::{PgPool, Postgres, Row, Transaction};

const ADMIN_HTML: &str = include_str!("../../web/admin/index.html");
const ADMIN_CSS: &str = include_str!("../../web/admin/styles.css");
const ADMIN_JS: &str = include_str!("../../web/admin/app.js");

pub fn ui_routes() -> Router {
    Router::new()
        .push(Router::with_path("admin-console").get(admin_html))
        .push(Router::with_path("admin-console/styles.css").get(admin_css))
        .push(Router::with_path("admin-console/app.js").get(admin_js))
}

#[handler]
async fn admin_html(res: &mut Response) {
    res.render(Text::Html(ADMIN_HTML));
}

#[handler]
async fn admin_css(res: &mut Response) {
    res.render(Text::Css(ADMIN_CSS));
}

#[handler]
async fn admin_js(res: &mut Response) {
    res.render(Text::Js(ADMIN_JS));
}

pub fn routes() -> Router {
    Router::with_path("admin")
        .hoop(require_mutation_token)
        .push(Router::with_path("periods/statuses").get(project_statuses))
        .push(
            Router::with_path("projects/{project_id}/periods")
                .get(list_project_periods)
                .post(create_project_period),
        )
        .push(
            Router::with_path("projects/{project_id}/periods/{activity_period_id}")
                .get(get_project_period)
                .put(replace_project_period),
        )
        .push(
            Router::with_path("projects/{project_id}/periods/{activity_period_id}/status")
                .patch(update_project_period_status),
        )
        .push(
            Router::with_path("projects")
                .get(list_master_projects)
                .post(create_master_project),
        )
        .push(
            Router::with_path("projects/{project_id}")
                .get(get_master_project)
                .patch(update_master_project),
        )
        .push(
            Router::with_path("projects/{project_id}/notification")
                .get(get_project_notification)
                .patch(update_project_notification),
        )
        .push(
            Router::with_path("projects/{project_id}/accounts")
                .get(list_project_accounts)
                .post(create_project_account),
        )
        .push(
            Router::with_path("projects/{project_id}/accounts/{xingtu_account_id}")
                .patch(update_project_account),
        )
        .push(
            Router::with_path("projects/{project_id}/auditors")
                .get(list_project_auditors)
                .post(create_project_auditor),
        )
        .push(
            Router::with_path("projects/{project_id}/auditors/{project_auditor_id}")
                .patch(update_project_auditor),
        )
        .push(Router::with_path("card-messages").get(list_card_messages))
        .push(Router::with_path("card-messages/{message_id}/recall").post(recall_card_message))
        .push(Router::with_path("workflow-runs").get(list_workflow_runs))
        .push(Router::with_path("workflow-runs/{workflow_run_id}/steps").get(list_workflow_steps))
        .push(Router::with_path("failed-sources").get(list_failed_sources))
        .push(
            Router::with_path("failed-sources/{feishu_source_id}/retry").post(retry_failed_source),
        )
        .push(
            Router::with_path("failed-sources/{feishu_source_id}/ignore")
                .post(ignore_failed_source),
        )
        .push(Router::with_path("quarantine").get(list_quarantine))
        .push(Router::with_path("status").get(system_status))
        .push(Router::with_path("xingtu/session-upload-token").get(get_xingtu_session_upload_token))
        .push(crate::server::admin_sheet::routes())
        .push(Router::with_path("feishu/bitable/tables").get(list_bitable_tables))
        .push(Router::with_path("feishu/chats").get(list_bot_chats))
        .push(Router::with_path("feishu/chats/{chat_id}/members").get(list_chat_members))
}

#[derive(Debug, Deserialize, Serialize, ToParameters, ToSchema)]
#[salvo(parameters(default_parameter_in = Query))]
struct AdminListQuery {
    /// 按主项目 ID 过滤；优先于 project。
    project_id: Option<i64>,
    /// 按项目过滤，例如 ROK。
    project: Option<String>,
    /// 是否包含停用记录，默认 true。
    include_inactive: Option<bool>,
}

#[derive(Debug, Deserialize, Serialize, ToParameters, ToSchema)]
#[salvo(parameters(default_parameter_in = Path))]
struct ProjectPath {
    project_id: i64,
}

#[derive(Debug, Deserialize, Serialize, ToParameters, ToSchema)]
#[salvo(parameters(default_parameter_in = Path))]
struct ProjectPeriodPath {
    project_id: i64,
    activity_period_id: i64,
}

#[derive(Debug, Deserialize, Serialize, ToParameters, ToSchema)]
#[salvo(parameters(default_parameter_in = Path))]
struct ProjectAccountPath {
    project_id: i64,
    xingtu_account_id: String,
}

#[derive(Debug, Deserialize, Serialize, ToParameters, ToSchema)]
#[salvo(parameters(default_parameter_in = Path))]
struct ProjectAuditorPath {
    project_id: i64,
    project_auditor_id: i64,
}

#[derive(Debug, Default, Deserialize, Serialize, ToParameters, ToSchema)]
#[salvo(parameters(default_parameter_in = Query))]
struct PeriodListQuery {
    /// 是否包含停用期次，默认 true。
    include_inactive: Option<bool>,
}

#[derive(Debug, Deserialize, Serialize, ToParameters, ToSchema)]
#[salvo(parameters(default_parameter_in = Path))]
struct CardMessagePath {
    /// 飞书消息 ID。
    message_id: String,
}

#[derive(Debug, Deserialize, Serialize, ToParameters, ToSchema)]
#[salvo(parameters(default_parameter_in = Path))]
struct WorkflowRunPath {
    workflow_run_id: i64,
}

#[derive(Debug, Deserialize, Serialize, ToParameters, ToSchema)]
#[salvo(parameters(default_parameter_in = Path))]
struct FeishuSourcePath {
    feishu_source_id: i64,
}

#[derive(Debug, Deserialize, Serialize, ToParameters, ToSchema)]
#[salvo(parameters(default_parameter_in = Query))]
struct OperationalListQuery {
    limit: Option<i64>,
    offset: Option<i64>,
}

#[derive(Debug, Default, Deserialize, Serialize, ToParameters, ToSchema)]
#[salvo(parameters(default_parameter_in = Query))]
struct FeishuChatListQuery {
    /// 群主用户 ID 类型：open_id、union_id 或 user_id。缺省时使用飞书默认值。
    user_id_type: Option<String>,
    /// 排序方式：ByCreateTimeAsc 或 ByActiveTimeDesc。
    sort_type: Option<String>,
    /// 单页数量，范围 1..=100，飞书默认 20。
    page_size: Option<i32>,
    /// 飞书上一页响应返回的分页标记。
    page_token: Option<String>,
}

#[derive(Debug, Default, Deserialize, Serialize, ToParameters, ToSchema)]
#[salvo(parameters(default_parameter_in = Query))]
struct FeishuChatMembersQuery {
    /// 成员 ID 类型：open_id、union_id 或 user_id。缺省时使用飞书默认值。
    member_id_type: Option<String>,
    /// 单页数量，范围 1..=100，飞书默认 20。
    page_size: Option<i32>,
    /// 飞书上一页响应返回的分页标记。
    page_token: Option<String>,
}

#[derive(Debug, Deserialize, Serialize, ToParameters, ToSchema)]
#[salvo(parameters(default_parameter_in = Path))]
struct FeishuChatPath {
    /// 飞书群聊 ID，以 oc_ 开头。
    chat_id: String,
}

#[derive(Debug, Default, Deserialize, Serialize, ToParameters, ToSchema)]
#[salvo(parameters(default_parameter_in = Query))]
struct FeishuBitableTablesQuery {
    /// 飞书知识库多维表或普通多维表链接，也兼容 Markdown 链接文本。
    url: String,
    /// 飞书上一页响应返回的分页标记。
    page_token: Option<String>,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
struct FailedSourceDto {
    feishu_source_id: i64,
    activity_period_id: i64,
    project: String,
    period: String,
    content_type: String,
    feishu_sheet_url: String,
    import_status: String,
    attempt_count: i32,
    last_attempt_at: Option<DateTime<Utc>>,
    next_retry_at: Option<DateTime<Utc>>,
    dead_letter_at: Option<DateTime<Utc>>,
    ignored_at: Option<DateTime<Utc>>,
    error_message: Option<String>,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
struct QuarantineDto {
    quarantine_id: i64,
    feishu_source_id: Option<i64>,
    content_config_id: i64,
    content_type: String,
    unique_key: String,
    reason_code: String,
    reason_message: String,
    created_at: DateTime<Utc>,
    resolved_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
struct SystemStatusDto {
    running_workflows: i64,
    failed_sources: i64,
    dead_letter_sources: i64,
    unresolved_quarantine_rows: i64,
    invalid_accounts: i64,
    latest_success_at: Option<DateTime<Utc>>,
    latest_failure_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
struct XingtuExtensionConfigDto {
    /// 插件上传登录态时使用的共享 Bearer Token；不是数据库登录态加密密钥。
    xingtu_session_upload_token: String,
    /// 插件上传登录态的固定 API 路径。
    upload_path: &'static str,
    /// HTTP Authorization 请求头使用的认证类型。
    token_type: &'static str,
}

#[endpoint(
    tags("admin"),
    summary = "获取星图同步插件上传 Token",
    description = "通过 MUTATION_API_TOKEN 鉴权返回 XINGTU_SESSION_UPLOAD_TOKEN，供内部前端按星图账号 ID 生成同步插件包。该 Token 仅用于上传登录态，不是数据库登录态加密密钥。"
)]
async fn get_xingtu_session_upload_token(
    res: &mut Response,
) -> ApiResult<XingtuExtensionConfigDto> {
    build_xingtu_extension_config(configured_xingtu_session_upload_token(), res)
}

fn build_xingtu_extension_config(
    session_upload_token: Option<String>,
    res: &mut Response,
) -> ApiResult<XingtuExtensionConfigDto> {
    res.headers_mut()
        .insert(CACHE_CONTROL, HeaderValue::from_static("no-store, private"));
    res.headers_mut()
        .insert(PRAGMA, HeaderValue::from_static("no-cache"));
    let session_upload_token = session_upload_token.ok_or_else(|| {
        ApiError::service_unavailable(
            "服务端未配置 XINGTU_SESSION_UPLOAD_TOKEN，暂时无法生成星图同步插件配置",
        )
    })?;

    Ok(Json(XingtuExtensionConfigDto {
        xingtu_session_upload_token: session_upload_token,
        upload_path: "/api/v1/xingtu/sessions",
        token_type: "Bearer",
    }))
}

#[derive(Debug, Clone, Serialize, ToSchema)]
struct ProjectStatusDto {
    activity_period_id: i64,
    project: String,
    period: String,
    account_status: Option<String>,
    latest_success_at: Option<DateTime<Utc>>,
    latest_failure_at: Option<DateTime<Utc>>,
    latest_data_at: Option<DateTime<Utc>>,
    pending_sources: i64,
    failed_sources: i64,
    quarantine_rows: i64,
}

#[derive(Debug, Deserialize, Serialize, ToParameters, ToSchema)]
#[salvo(parameters(default_parameter_in = Query))]
struct CardMessageHistoryQuery {
    /// 消息类别：error_log、audit、daily_report。
    category: Option<String>,
    /// 北京时间发送日期起点，闭区间。
    date_from: Option<NaiveDate>,
    /// 北京时间发送日期终点，闭区间。
    date_to: Option<NaiveDate>,
    /// 返回条数，限制为 1..500，默认 50。
    limit: Option<i64>,
    /// 分页偏移量，默认 0。
    offset: Option<i64>,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct ProjectAuditorDto {
    pub project_auditor_id: i64,
    pub project_id: i64,
    pub auditor_name: String,
    pub auditor_id: String,
    pub auditor_id_type: String,
    pub sort_order: i32,
    pub is_active: bool,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct MasterProjectDto {
    pub project_id: i64,
    pub project_key: String,
    pub display_name: String,
    pub is_active: bool,
    pub remark: Option<String>,
    pub account_count: i64,
    pub auditor_count: i64,
    pub period_count: i64,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct ProjectNotificationDto {
    pub project_id: i64,
    pub notification_receive_id_type: String,
    pub notification_receive_id: String,
    pub audit_notice_card_template_id: String,
    pub audit_result_field: String,
    pub login_notice_card_template_id: String,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct MasterProjectAccountDto {
    pub xingtu_account_id: String,
    pub project_id: i64,
    pub display_name: Option<String>,
    pub ops_ids: Vec<String>,
    pub login_check_enabled: bool,
    pub session_status: String,
    pub is_default: bool,
    pub is_active: bool,
    pub remark: Option<String>,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct MasterProjectDetailDto {
    #[serde(flatten)]
    pub project: MasterProjectDto,
    pub notification: ProjectNotificationDto,
    pub accounts: Vec<MasterProjectAccountDto>,
    pub auditors: Vec<ProjectAuditorDto>,
    pub periods: Vec<ActivityAdminDto>,
}

#[derive(Debug, Deserialize, Serialize, ToSchema)]
#[serde(deny_unknown_fields)]
struct CreateMasterProjectRequest {
    project_key: String,
    display_name: String,
    notification: ProjectNotificationInput,
    #[serde(default = "default_true")]
    is_active: bool,
    remark: Option<String>,
}

#[derive(Debug, Deserialize, Serialize, ToSchema)]
#[serde(deny_unknown_fields)]
struct UpdateMasterProjectRequest {
    display_name: Option<String>,
    is_active: Option<bool>,
    remark: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize, ToSchema)]
#[serde(deny_unknown_fields)]
struct ProjectNotificationInput {
    notification_receive_id_type: String,
    notification_receive_id: String,
    audit_notice_card_template_id: String,
    audit_result_field: String,
    login_notice_card_template_id: String,
}

#[derive(Debug, Clone, Deserialize, Serialize, ToSchema)]
#[serde(deny_unknown_fields)]
struct UpdateProjectNotificationRequest {
    notification_receive_id_type: Option<String>,
    notification_receive_id: Option<String>,
    audit_notice_card_template_id: Option<String>,
    audit_result_field: Option<String>,
    login_notice_card_template_id: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize, ToSchema)]
#[serde(deny_unknown_fields)]
struct UpsertProjectPeriodRequest {
    period: String,
    period_code: Option<String>,
    /// 主项目已有账号 ID；不传时使用项目默认账号。
    xingtu_account_id: Option<String>,
    task_month: NaiveDate,
    bitable_url: String,
    cpm_table_id: Option<String>,
    #[serde(default = "default_true")]
    need_trace: bool,
    tracking_start_date: Option<NaiveDate>,
    tracking_end_date: Option<NaiveDate>,
    #[serde(default)]
    workflows: WorkflowConfig,
    contents: Vec<ActivityContentConfig>,
    #[serde(default = "default_true")]
    is_active: bool,
    remark: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize, ToSchema)]
#[serde(deny_unknown_fields)]
struct MasterProjectAccountInput {
    xingtu_account_id: String,
    display_name: Option<String>,
    #[serde(default)]
    ops_ids: Vec<String>,
    #[serde(default = "default_true")]
    login_check_enabled: bool,
    #[serde(default)]
    is_default: bool,
    #[serde(default = "default_true")]
    is_active: bool,
    remark: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize, ToSchema)]
#[serde(deny_unknown_fields)]
struct UpdateMasterProjectAccountRequest {
    display_name: Option<String>,
    ops_ids: Option<Vec<String>>,
    login_check_enabled: Option<bool>,
    is_default: Option<bool>,
    is_active: Option<bool>,
    remark: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize, ToSchema)]
#[serde(deny_unknown_fields)]
struct MasterProjectAuditorInput {
    auditor_name: String,
    auditor_id: String,
    #[serde(default = "default_user_id_type")]
    auditor_id_type: String,
    #[serde(default)]
    sort_order: i32,
    #[serde(default = "default_true")]
    is_active: bool,
}

#[derive(Debug, Deserialize, Serialize, ToSchema)]
#[serde(deny_unknown_fields)]
struct UpdateAuditorRequest {
    auditor_name: Option<String>,
    auditor_id_type: Option<String>,
    sort_order: Option<i32>,
    is_active: Option<bool>,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct ActivityAdminDto {
    pub activity_period_id: i64,
    pub project_id: i64,
    pub project_key: String,
    pub project_display_name: String,
    pub period: String,
    pub period_code: Option<String>,
    pub xingtu_account_id: Option<String>,
    pub task_month: NaiveDate,
    pub bitable_url: String,
    pub cpm_table_id: Option<String>,
    pub need_trace: bool,
    pub morning_review_enabled: bool,
    pub periodic_sync_enabled: bool,
    pub periodic_sync_interval_hours: i32,
    pub tracking_start_date: Option<NaiveDate>,
    pub tracking_end_date: Option<NaiveDate>,
    pub is_active: bool,
    pub remark: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct ActivityContentAdminDto {
    pub content_config_id: i64,
    pub content_type: String,
    pub xingtu_task_id: String,
    pub xingtu_task_name: Option<String>,
    pub source_spreadsheet_url: Option<String>,
    pub source_spreadsheet_url_update_mode: String,
    pub manual_table_id: Option<String>,
    pub main_table_id: String,
    pub audit_table_id: String,
    pub data_source_field: String,
    pub spreadsheet_source_value: String,
    pub manual_source_value: String,
    pub manual_overrides_spreadsheet: bool,
    pub manual_auto_approve: bool,
    pub manual_auto_approve_result: String,
    pub sync_enabled: bool,
    pub trace_enabled: bool,
    pub remark: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct ActivityDetailDto {
    pub activity: ActivityAdminDto,
    pub contents: Vec<ActivityContentAdminDto>,
}

#[derive(Debug, Deserialize, Serialize, ToSchema)]
struct UpdateActivityStatusRequest {
    is_active: Option<bool>,
    need_trace: Option<bool>,
    morning_review_enabled: Option<bool>,
    periodic_sync_enabled: Option<bool>,
}

#[endpoint(tags("admin"), summary = "查询主项目配置")]
async fn list_master_projects(
    query: AdminListQuery,
    depot: &mut Depot,
) -> ApiResult<Vec<MasterProjectDto>> {
    let state = state_from_depot(depot)?;
    let rows = sqlx::query(master_project_select_sql())
        .bind(query.project_id)
        .bind(trimmed_optional(query.project))
        .bind(query.include_inactive.unwrap_or(true))
        .fetch_all(&state.pool)
        .await?;
    Ok(Json(
        rows.into_iter()
            .map(master_project_from_row)
            .collect::<Result<Vec<_>, _>>()?,
    ))
}

#[endpoint(tags("admin"), summary = "查询主项目及账号、审核员、期次")]
async fn get_master_project(
    path: ProjectPath,
    depot: &mut Depot,
) -> ApiResult<MasterProjectDetailDto> {
    let state = state_from_depot(depot)?;
    Ok(Json(
        fetch_master_project_detail(&state.pool, path.project_id).await?,
    ))
}

#[endpoint(tags("admin"), summary = "创建主项目")]
async fn create_master_project(
    body: RequiredJsonBody<CreateMasterProjectRequest>,
    depot: &mut Depot,
) -> ApiResult<MasterProjectDetailDto> {
    let state = state_from_depot(depot)?;
    let body = body.into_inner();
    let project_key = required_text(&body.project_key, "project_key")?;
    let display_name = required_text(&body.display_name, "display_name")?;
    validate_project_notification(&body.notification)?;
    let remark = trimmed_optional(body.remark.clone());
    let project_id: i64 = sqlx::query_scalar(
        r#"
        INSERT INTO xingtu_project (
            project_key, display_name, notification_receive_id_type,
            notification_receive_id, audit_notice_card_template_id,
            audit_result_field, login_notice_card_template_id, is_active, remark
        )
        VALUES ($1, $2, $3::xingtu_receive_id_type, $4, $5, $6, $7, $8, $9)
        RETURNING project_id
        "#,
    )
    .bind(&project_key)
    .bind(display_name)
    .bind(body.notification.notification_receive_id_type.trim())
    .bind(body.notification.notification_receive_id.trim())
    .bind(body.notification.audit_notice_card_template_id.trim())
    .bind(body.notification.audit_result_field.trim())
    .bind(body.notification.login_notice_card_template_id.trim())
    .bind(body.is_active)
    .bind(remark)
    .fetch_one(&state.pool)
    .await?;
    state.query_cache.invalidate_all_shared().await?;
    Ok(Json(
        fetch_master_project_detail(&state.pool, project_id).await?,
    ))
}

#[endpoint(tags("admin"), summary = "更新主项目")]
async fn update_master_project(
    path: ProjectPath,
    body: RequiredJsonBody<UpdateMasterProjectRequest>,
    depot: &mut Depot,
) -> ApiResult<MasterProjectDetailDto> {
    let state = state_from_depot(depot)?;
    let body = body.into_inner();
    let remark_provided = body.remark.is_some();
    if body.display_name.is_none() && body.is_active.is_none() && !remark_provided {
        return Err(ApiError::bad_request("至少提供一个需要更新的项目字段"));
    }
    let display_name = body
        .display_name
        .as_deref()
        .map(|value| required_text(value, "display_name"))
        .transpose()?;
    let mut tx = state.pool.begin().await?;
    lock_project(&mut tx, path.project_id, false).await?;
    if body.is_active == Some(false) {
        let active_periods: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM xingtu_activity_period WHERE project_id = $1 AND is_active = true",
        )
        .bind(path.project_id)
        .fetch_one(&mut *tx)
        .await?;
        if active_periods > 0 {
            return Err(ApiError::conflict("主项目仍有启用期次，请先停用期次"));
        }
    }
    sqlx::query(
        r#"
        UPDATE xingtu_project
        SET display_name = COALESCE($2, display_name),
            is_active = COALESCE($3, is_active),
            remark = CASE WHEN $4 THEN $5 ELSE remark END
        WHERE project_id = $1
        "#,
    )
    .bind(path.project_id)
    .bind(display_name)
    .bind(body.is_active)
    .bind(remark_provided)
    .bind(trimmed_optional(body.remark))
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    state.query_cache.invalidate_all_shared().await?;
    Ok(Json(
        fetch_master_project_detail(&state.pool, path.project_id).await?,
    ))
}

#[endpoint(tags("admin"), summary = "查询项目通知配置")]
async fn get_project_notification(
    path: ProjectPath,
    depot: &mut Depot,
) -> ApiResult<ProjectNotificationDto> {
    let state = state_from_depot(depot)?;
    Ok(Json(
        fetch_project_notification(&state.pool, path.project_id).await?,
    ))
}

#[endpoint(tags("admin"), summary = "更新项目通知配置")]
async fn update_project_notification(
    path: ProjectPath,
    body: RequiredJsonBody<UpdateProjectNotificationRequest>,
    depot: &mut Depot,
) -> ApiResult<ProjectNotificationDto> {
    let state = state_from_depot(depot)?;
    let body = body.into_inner();
    if body.notification_receive_id_type.is_none()
        && body.notification_receive_id.is_none()
        && body.audit_notice_card_template_id.is_none()
        && body.audit_result_field.is_none()
        && body.login_notice_card_template_id.is_none()
    {
        return Err(ApiError::bad_request("至少提供一个需要更新的通知字段"));
    }
    if let Some(value) = body.notification_receive_id_type.as_deref() {
        validate_receive_id_type(value)?;
    }
    let mut tx = state.pool.begin().await?;
    lock_project(&mut tx, path.project_id, false).await?;
    sqlx::query(
        r#"
        UPDATE xingtu_project SET
            notification_receive_id_type = COALESCE($2::xingtu_receive_id_type, notification_receive_id_type),
            notification_receive_id = COALESCE($3, notification_receive_id),
            audit_notice_card_template_id = COALESCE($4, audit_notice_card_template_id),
            audit_result_field = COALESCE($5, audit_result_field),
            login_notice_card_template_id = COALESCE($6, login_notice_card_template_id)
        WHERE project_id = $1
        "#,
    )
    .bind(path.project_id)
    .bind(body.notification_receive_id_type.map(|v| v.trim().to_owned()))
    .bind(required_optional_text(body.notification_receive_id, "notification_receive_id")?)
    .bind(required_optional_text(body.audit_notice_card_template_id, "audit_notice_card_template_id")?)
    .bind(required_optional_text(body.audit_result_field, "audit_result_field")?)
    .bind(required_optional_text(body.login_notice_card_template_id, "login_notice_card_template_id")?)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    state.query_cache.invalidate_all_shared().await?;
    Ok(Json(
        fetch_project_notification(&state.pool, path.project_id).await?,
    ))
}

#[endpoint(tags("admin"), summary = "查询项目星图账号")]
async fn list_project_accounts(
    path: ProjectPath,
    query: PeriodListQuery,
    depot: &mut Depot,
) -> ApiResult<Vec<MasterProjectAccountDto>> {
    let state = state_from_depot(depot)?;
    ensure_project(&state.pool, path.project_id).await?;
    let rows = sqlx::query(project_account_select_sql())
        .bind(path.project_id)
        .bind(query.include_inactive.unwrap_or(true))
        .fetch_all(&state.pool)
        .await?;
    Ok(Json(
        rows.into_iter()
            .map(master_project_account_from_row)
            .collect::<Result<_, _>>()?,
    ))
}

#[endpoint(tags("admin"), summary = "新增项目星图账号")]
async fn create_project_account(
    path: ProjectPath,
    body: RequiredJsonBody<MasterProjectAccountInput>,
    depot: &mut Depot,
) -> ApiResult<MasterProjectAccountDto> {
    let state = state_from_depot(depot)?;
    let body = body.into_inner();
    let account_id = required_text(&body.xingtu_account_id, "xingtu_account_id")?;
    let mut tx = state.pool.begin().await?;
    lock_project(&mut tx, path.project_id, true).await?;
    let has_default: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM xingtu_project_account WHERE project_id = $1 AND is_default = true)",
    )
            .bind(path.project_id)
            .fetch_one(&mut *tx)
            .await?;
    let is_default = body.is_default || !has_default;
    if is_default && !body.is_active {
        return Err(ApiError::bad_request("默认账号必须启用"));
    }
    let ops_ids = normalize_string_list(body.ops_ids);
    if is_default {
        sqlx::query("UPDATE xingtu_project_account SET is_default = false WHERE project_id = $1")
            .bind(path.project_id)
            .execute(&mut *tx)
            .await?;
    }
    sqlx::query(
        r#"
        INSERT INTO xingtu_project_account (
            xingtu_account_id, project_id, display_name, ops_ids,
            login_check_enabled, is_default, is_active, remark
        ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
        "#,
    )
    .bind(&account_id)
    .bind(path.project_id)
    .bind(trimmed_optional(body.display_name))
    .bind(ops_ids)
    .bind(body.login_check_enabled)
    .bind(is_default)
    .bind(body.is_active)
    .bind(trimmed_optional(body.remark))
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    state.query_cache.invalidate_all_shared().await?;
    Ok(Json(
        fetch_project_account(&state.pool, path.project_id, &account_id).await?,
    ))
}

#[endpoint(tags("admin"), summary = "更新项目星图账号")]
async fn update_project_account(
    path: ProjectAccountPath,
    body: RequiredJsonBody<UpdateMasterProjectAccountRequest>,
    depot: &mut Depot,
) -> ApiResult<MasterProjectAccountDto> {
    let state = state_from_depot(depot)?;
    let body = body.into_inner();
    let display_name_provided = body.display_name.is_some();
    let remark_provided = body.remark.is_some();
    if !display_name_provided
        && body.ops_ids.is_none()
        && body.login_check_enabled.is_none()
        && body.is_default.is_none()
        && body.is_active.is_none()
        && !remark_provided
    {
        return Err(ApiError::bad_request("至少提供一个需要更新的账号字段"));
    }
    if body.is_default == Some(true) && body.is_active == Some(false) {
        return Err(ApiError::bad_request("默认账号必须启用"));
    }
    if body.is_default == Some(false) {
        return Err(ApiError::bad_request(
            "不能直接取消默认账号；请把另一个启用账号设为默认",
        ));
    }
    let mut tx = state.pool.begin().await?;
    lock_project(&mut tx, path.project_id, false).await?;
    let (current_default, current_active): (bool, bool) = sqlx::query_as(
        "SELECT is_default, is_active FROM xingtu_project_account WHERE project_id = $1 AND xingtu_account_id = $2 FOR UPDATE",
    )
    .bind(path.project_id)
    .bind(&path.xingtu_account_id)
    .fetch_optional(&mut *tx)
    .await?
    .ok_or_else(|| ApiError::not_found("该项目下不存在指定星图账号"))?;
    if body.is_active == Some(false) {
        let references: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM xingtu_activity_period WHERE project_id = $1 AND xingtu_account_id = $2 AND is_active = true",
        )
        .bind(path.project_id)
        .bind(&path.xingtu_account_id)
        .fetch_one(&mut *tx)
        .await?;
        if references > 0 {
            return Err(ApiError::conflict("账号仍被启用期次使用，请先调整期次账号"));
        }
        if current_default {
            return Err(ApiError::conflict("不能停用默认账号，请先设置其他默认账号"));
        }
    }
    if body.is_default == Some(true) {
        if !body.is_active.unwrap_or(current_active) {
            return Err(ApiError::bad_request("默认账号必须启用"));
        }
        sqlx::query("UPDATE xingtu_project_account SET is_default = false WHERE project_id = $1")
            .bind(path.project_id)
            .execute(&mut *tx)
            .await?;
    }
    let ops_ids = body.ops_ids.map(normalize_string_list);
    sqlx::query(
        r#"
        UPDATE xingtu_project_account SET
            display_name = CASE WHEN $3 THEN $4 ELSE display_name END,
            ops_ids = COALESCE($5, ops_ids), login_check_enabled = COALESCE($6, login_check_enabled),
            is_default = CASE WHEN $7::boolean = true THEN true ELSE is_default END,
            is_active = COALESCE($8, is_active), remark = CASE WHEN $9 THEN $10 ELSE remark END
        WHERE project_id = $1 AND xingtu_account_id = $2
        "#,
    )
    .bind(path.project_id)
    .bind(&path.xingtu_account_id)
    .bind(display_name_provided)
    .bind(trimmed_optional(body.display_name))
    .bind(ops_ids)
    .bind(body.login_check_enabled)
    .bind(body.is_default)
    .bind(body.is_active)
    .bind(remark_provided)
    .bind(trimmed_optional(body.remark))
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    state.query_cache.invalidate_all_shared().await?;
    Ok(Json(
        fetch_project_account(&state.pool, path.project_id, &path.xingtu_account_id).await?,
    ))
}

#[endpoint(tags("admin"), summary = "查询项目审核员")]
async fn list_project_auditors(
    path: ProjectPath,
    query: PeriodListQuery,
    depot: &mut Depot,
) -> ApiResult<Vec<ProjectAuditorDto>> {
    let state = state_from_depot(depot)?;
    ensure_project(&state.pool, path.project_id).await?;
    let rows = sqlx::query(project_auditor_select_sql())
        .bind(path.project_id)
        .bind(query.include_inactive.unwrap_or(true))
        .fetch_all(&state.pool)
        .await?;
    Ok(Json(
        rows.into_iter()
            .map(project_auditor_from_row)
            .collect::<Result<_, _>>()?,
    ))
}

#[endpoint(tags("admin"), summary = "新增项目审核员")]
async fn create_project_auditor(
    path: ProjectPath,
    body: RequiredJsonBody<MasterProjectAuditorInput>,
    depot: &mut Depot,
) -> ApiResult<ProjectAuditorDto> {
    let state = state_from_depot(depot)?;
    let body = body.into_inner();
    let name = required_text(&body.auditor_name, "auditor_name")?;
    let auditor_id = required_text(&body.auditor_id, "auditor_id")?;
    validate_auditor_id_type(&auditor_id, &body.auditor_id_type)?;
    let mut tx = state.pool.begin().await?;
    lock_project(&mut tx, path.project_id, true).await?;
    let id: i64 = sqlx::query_scalar(
        r#"
        INSERT INTO xingtu_project_auditor (
            project_id, auditor_name, auditor_id, auditor_id_type, sort_order, is_active
        ) VALUES ($1, $2, $3, $4::xingtu_receive_id_type, $5, $6)
        RETURNING project_auditor_id
        "#,
    )
    .bind(path.project_id)
    .bind(name)
    .bind(auditor_id)
    .bind(body.auditor_id_type.trim())
    .bind(body.sort_order)
    .bind(body.is_active)
    .fetch_one(&mut *tx)
    .await
    .map_err(map_project_auditor_insert_error)?;
    tx.commit().await?;
    state.query_cache.invalidate_all_shared().await?;
    Ok(Json(
        fetch_project_auditor(&state.pool, path.project_id, id).await?,
    ))
}

#[endpoint(tags("admin"), summary = "更新项目审核员")]
async fn update_project_auditor(
    path: ProjectAuditorPath,
    body: RequiredJsonBody<UpdateAuditorRequest>,
    depot: &mut Depot,
) -> ApiResult<ProjectAuditorDto> {
    let state = state_from_depot(depot)?;
    let body = body.into_inner();
    validate_auditor_update(&body)?;
    let name = body
        .auditor_name
        .as_deref()
        .map(|v| required_text(v, "auditor_name"))
        .transpose()?;
    if let Some(value) = body.auditor_id_type.as_deref() {
        let auditor_id: String = sqlx::query_scalar(
            "SELECT auditor_id FROM xingtu_project_auditor WHERE project_id = $1 AND project_auditor_id = $2",
        )
        .bind(path.project_id)
        .bind(path.project_auditor_id)
        .fetch_optional(&state.pool)
        .await?
        .ok_or_else(|| ApiError::not_found("该项目下不存在指定审核员"))?;
        validate_auditor_id_type(&auditor_id, value)?;
    }
    let row = sqlx::query(
        r#"
        UPDATE xingtu_project_auditor SET auditor_name = COALESCE($3, auditor_name),
            auditor_id_type = COALESCE($4::xingtu_receive_id_type, auditor_id_type),
            sort_order = COALESCE($5, sort_order), is_active = COALESCE($6, is_active)
        WHERE project_id = $1 AND project_auditor_id = $2 RETURNING project_auditor_id
        "#,
    )
    .bind(path.project_id)
    .bind(path.project_auditor_id)
    .bind(name)
    .bind(body.auditor_id_type.map(|v| v.trim().to_owned()))
    .bind(body.sort_order)
    .bind(body.is_active)
    .fetch_optional(&state.pool)
    .await?
    .ok_or_else(|| ApiError::not_found("该项目下不存在指定审核员"))?;
    let id: i64 = row.try_get("project_auditor_id")?;
    state.query_cache.invalidate_all_shared().await?;
    Ok(Json(
        fetch_project_auditor(&state.pool, path.project_id, id).await?,
    ))
}

#[endpoint(tags("admin"), summary = "查询项目期次")]
async fn list_project_periods(
    path: ProjectPath,
    query: PeriodListQuery,
    depot: &mut Depot,
) -> ApiResult<Vec<ActivityAdminDto>> {
    let state = state_from_depot(depot)?;
    ensure_project(&state.pool, path.project_id).await?;
    let rows = sqlx::query(&format!(
        "{} WHERE period.project_id = $1 AND ($2::boolean OR period.is_active = true) ORDER BY period.task_month DESC, period.activity_period_id DESC",
        activity_select_sql()
    ))
    .bind(path.project_id).bind(query.include_inactive.unwrap_or(true))
    .fetch_all(&state.pool).await?;
    Ok(Json(
        rows.into_iter()
            .map(activity_from_row)
            .collect::<Result<_, _>>()?,
    ))
}

#[endpoint(tags("admin"), summary = "查询项目期次详情")]
async fn get_project_period(
    path: ProjectPeriodPath,
    depot: &mut Depot,
) -> ApiResult<ActivityDetailDto> {
    let state = state_from_depot(depot)?;
    Ok(Json(
        fetch_activity_detail(&state.pool, path.project_id, path.activity_period_id).await?,
    ))
}

#[endpoint(tags("admin"), summary = "新增项目期次")]
async fn create_project_period(
    path: ProjectPath,
    body: RequiredJsonBody<UpsertProjectPeriodRequest>,
    depot: &mut Depot,
) -> ApiResult<ActivityDetailDto> {
    save_project_period(path.project_id, None, body.into_inner(), depot).await
}

#[endpoint(tags("admin"), summary = "完整替换项目期次")]
async fn replace_project_period(
    path: ProjectPeriodPath,
    body: RequiredJsonBody<UpsertProjectPeriodRequest>,
    depot: &mut Depot,
) -> ApiResult<ActivityDetailDto> {
    save_project_period(
        path.project_id,
        Some(path.activity_period_id),
        body.into_inner(),
        depot,
    )
    .await
}

async fn save_project_period(
    project_id: i64,
    activity_period_id: Option<i64>,
    body: UpsertProjectPeriodRequest,
    depot: &mut Depot,
) -> ApiResult<ActivityDetailDto> {
    validate_project_period_request(&body)?;
    let state = state_from_depot(depot)?;
    let mut tx = state.pool.begin().await?;
    lock_project(&mut tx, project_id, true).await?;
    let account_id =
        resolve_project_account(&mut tx, project_id, body.xingtu_account_id.as_deref()).await?;
    let task_ids = body
        .contents
        .iter()
        .map(|content| content.xingtu_task.task_id.trim().to_owned())
        .collect::<Vec<_>>();
    let conflicting_tasks = sqlx::query_scalar::<_, String>(
        r#"
        SELECT content.xingtu_task_id
        FROM xingtu_activity_content_config content
        JOIN xingtu_activity_period period ON period.activity_period_id = content.activity_period_id
        WHERE content.xingtu_task_id = ANY($1::text[])
            AND ($2::bigint IS NULL OR period.activity_period_id <> $2)
        ORDER BY content.xingtu_task_id
        "#,
    )
    .bind(&task_ids)
    .bind(activity_period_id)
    .fetch_all(&mut *tx)
    .await?;
    if !conflicting_tasks.is_empty() {
        return Err(ApiError::conflict(format!(
            "星图任务已属于其他期次：{}",
            conflicting_tasks.join(", ")
        )));
    }
    let saved_id: i64 = if let Some(id) = activity_period_id {
        sqlx::query_scalar(
            r#"
            UPDATE xingtu_activity_period SET period=$3, period_code=$4, xingtu_account_id=$5,
                task_month=$6, bitable_url=$7, cpm_table_id=$8, need_trace=$9,
                morning_review_enabled=$10, periodic_sync_enabled=$11,
                periodic_sync_interval_hours=$12, tracking_start_date=$13,
                tracking_end_date=$14, is_active=$15, remark=$16
            WHERE project_id=$1 AND activity_period_id=$2 RETURNING activity_period_id
            "#,
        )
        .bind(project_id)
        .bind(id)
        .bind(body.period.trim())
        .bind(trimmed_optional(body.period_code.clone()))
        .bind(&account_id)
        .bind(body.task_month)
        .bind(body.bitable_url.trim())
        .bind(trimmed_optional(body.cpm_table_id.clone()))
        .bind(body.need_trace)
        .bind(body.workflows.morning_workflow_enabled)
        .bind(body.workflows.periodic_sync_enabled)
        .bind(body.workflows.periodic_sync_interval_hours)
        .bind(body.tracking_start_date)
        .bind(body.tracking_end_date)
        .bind(body.is_active)
        .bind(trimmed_optional(body.remark.clone()))
        .fetch_optional(&mut *tx)
        .await?
        .ok_or_else(|| ApiError::not_found("该项目下不存在指定期次"))?
    } else {
        sqlx::query_scalar(
            r#"
            INSERT INTO xingtu_activity_period (
                project_id, period, period_code, xingtu_account_id, task_month, bitable_url,
                cpm_table_id, need_trace, morning_review_enabled, periodic_sync_enabled,
                periodic_sync_interval_hours, tracking_start_date, tracking_end_date, is_active, remark
            ) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15)
            RETURNING activity_period_id
            "#,
        ).bind(project_id).bind(body.period.trim()).bind(trimmed_optional(body.period_code.clone()))
        .bind(&account_id).bind(body.task_month).bind(body.bitable_url.trim())
        .bind(trimmed_optional(body.cpm_table_id.clone())).bind(body.need_trace)
        .bind(body.workflows.morning_workflow_enabled).bind(body.workflows.periodic_sync_enabled)
        .bind(body.workflows.periodic_sync_interval_hours).bind(body.tracking_start_date)
        .bind(body.tracking_end_date).bind(body.is_active).bind(trimmed_optional(body.remark.clone()))
        .fetch_one(&mut *tx).await?
    };
    state
        .workflow
        .activity_repo
        .upsert_contents(&mut tx, saved_id, &body.contents)
        .await?;
    tx.commit().await?;
    state.query_cache.invalidate_all_shared().await?;
    Ok(Json(
        fetch_activity_detail(&state.pool, project_id, saved_id).await?,
    ))
}

#[endpoint(tags("admin"), summary = "更新项目期次状态")]
async fn update_project_period_status(
    path: ProjectPeriodPath,
    body: RequiredJsonBody<UpdateActivityStatusRequest>,
    depot: &mut Depot,
) -> ApiResult<ActivityAdminDto> {
    let state = state_from_depot(depot)?;
    let body = body.into_inner();
    validate_activity_status_update(&body)?;
    let updated: Option<i64> = sqlx::query_scalar(
        r#"
        UPDATE xingtu_activity_period SET is_active=COALESCE($3,is_active), need_trace=COALESCE($4,need_trace),
            morning_review_enabled=COALESCE($5,morning_review_enabled), periodic_sync_enabled=COALESCE($6,periodic_sync_enabled)
        WHERE project_id=$1 AND activity_period_id=$2 RETURNING activity_period_id
        "#,
    ).bind(path.project_id).bind(path.activity_period_id).bind(body.is_active).bind(body.need_trace)
    .bind(body.morning_review_enabled).bind(body.periodic_sync_enabled).fetch_optional(&state.pool).await?;
    updated.ok_or_else(|| ApiError::not_found("该项目下不存在指定期次"))?;
    state.query_cache.invalidate_all_shared().await?;
    Ok(Json(
        fetch_activity_detail(&state.pool, path.project_id, path.activity_period_id)
            .await?
            .activity,
    ))
}

#[endpoint(tags("admin"), summary = "查询卡片消息发送历史")]
async fn list_card_messages(
    query: CardMessageHistoryQuery,
    depot: &mut Depot,
) -> ApiResult<Vec<CardMessageHistory>> {
    validate_card_message_date_range(query.date_from, query.date_to)?;
    let category = query
        .category
        .as_deref()
        .map(CardMessageCategory::parse)
        .transpose()
        .map_err(|error| ApiError::bad_request(error.to_string()))?;
    let state = state_from_depot(depot)?;
    let repo = CardMessageHistoryRepository::new(state.pool.clone());
    let history = repo
        .list(CardMessageHistoryFilter {
            category,
            date_from: query.date_from,
            date_to: query.date_to,
            limit: query.limit.unwrap_or(50).clamp(1, 500),
            offset: query.offset.unwrap_or(0).max(0),
        })
        .await?;

    Ok(Json(history))
}

#[endpoint(tags("admin"), summary = "撤回已记录的飞书卡片消息")]
async fn recall_card_message(
    path: CardMessagePath,
    depot: &mut Depot,
) -> ApiResult<CardMessageHistory> {
    let message_id = required_text(&path.message_id, "message_id")?;
    let state = state_from_depot(depot)?;
    let repo = CardMessageHistoryRepository::new(state.pool.clone());
    let history = repo
        .find_by_message_id(&message_id)
        .await?
        .ok_or_else(|| ApiError::not_found(format!("卡片消息历史不存在：{message_id}")))?;

    if history.recalled_at.is_some() {
        return Ok(Json(history));
    }

    repo.mark_recall_started(&message_id).await?;
    if let Err(error) = FeishuImClient::new(&state.workflow.lark)
        .recall_message(&message_id)
        .await
    {
        let error_detail = format!("{error:#}");
        if let Err(history_error) = repo.mark_recall_failed(&message_id, &error_detail).await {
            tracing::error!(
                message_id,
                error = ?history_error,
                "记录卡片消息撤回失败原因时发生数据库错误"
            );
        }
        return Err(ApiError::internal(error_detail));
    }

    Ok(Json(repo.mark_recalled(&message_id).await?))
}

#[endpoint(tags("admin"), summary = "查询工作流运行历史")]
async fn list_workflow_runs(
    query: OperationalListQuery,
    depot: &mut Depot,
) -> ApiResult<Vec<WorkflowRunRecord>> {
    let state = state_from_depot(depot)?;
    Ok(Json(
        state
            .workflow
            .workflow_run_repo
            .list_runs(query.limit.unwrap_or(50), query.offset.unwrap_or(0))
            .await?,
    ))
}

#[endpoint(tags("admin"), summary = "查询工作流阶段历史")]
async fn list_workflow_steps(
    path: WorkflowRunPath,
    depot: &mut Depot,
) -> ApiResult<Vec<WorkflowStepRecord>> {
    let state = state_from_depot(depot)?;
    Ok(Json(
        state
            .workflow
            .workflow_run_repo
            .list_steps(path.workflow_run_id)
            .await?,
    ))
}

#[endpoint(tags("admin"), summary = "查询失败来源队列")]
async fn list_failed_sources(
    query: OperationalListQuery,
    depot: &mut Depot,
) -> ApiResult<Vec<FailedSourceDto>> {
    let state = state_from_depot(depot)?;
    let rows = sqlx::query(
        r#"
        SELECT s.feishu_source_id, c.activity_period_id, project.project_key AS project, a.period,
            s.content_type::text AS content_type, s.feishu_sheet_url,
            s.import_status::text AS import_status, s.attempt_count,
            s.last_attempt_at, s.next_retry_at, s.dead_letter_at, s.ignored_at,
            s.error_message
        FROM xingtu_feishu_source s
        JOIN xingtu_activity_content_config c ON c.content_config_id = s.content_config_id
        JOIN xingtu_activity_period a ON a.activity_period_id = c.activity_period_id
        JOIN xingtu_project project ON project.project_id = a.project_id
        WHERE s.is_imported = false AND s.import_status = 'failed'
        ORDER BY s.dead_letter_at DESC NULLS LAST, s.last_attempt_at DESC NULLS LAST, s.feishu_source_id
        LIMIT $1 OFFSET $2
        "#,
    )
    .bind(query.limit.unwrap_or(50).clamp(1, 500))
    .bind(query.offset.unwrap_or(0).max(0))
    .fetch_all(&state.pool)
    .await?;
    Ok(Json(
        rows.into_iter()
            .map(|row| {
                Ok(FailedSourceDto {
                    feishu_source_id: row.try_get("feishu_source_id")?,
                    activity_period_id: row.try_get("activity_period_id")?,
                    project: row.try_get("project")?,
                    period: row.try_get("period")?,
                    content_type: row.try_get("content_type")?,
                    feishu_sheet_url: row.try_get("feishu_sheet_url")?,
                    import_status: row.try_get("import_status")?,
                    attempt_count: row.try_get("attempt_count")?,
                    last_attempt_at: row.try_get("last_attempt_at")?,
                    next_retry_at: row.try_get("next_retry_at")?,
                    dead_letter_at: row.try_get("dead_letter_at")?,
                    ignored_at: row.try_get("ignored_at")?,
                    error_message: row.try_get("error_message")?,
                })
            })
            .collect::<Result<Vec<_>, sqlx::Error>>()?,
    ))
}

#[endpoint(tags("admin"), summary = "立即重试单个失败来源")]
async fn retry_failed_source(
    path: FeishuSourcePath,
    depot: &mut Depot,
) -> ApiResult<crate::xingtu::data_import::PendingImportResult> {
    let state = state_from_depot(depot)?;
    let request_id = crate::server::error::request_id_from_depot(depot);
    let result = state
        .workflow
        .retry_failed_source_only(path.feishu_source_id, "http", Some(&request_id))
        .await;
    match crate::server::cache::invalidate_after_write(&state.query_cache, result).await {
        Ok(result) => Ok(Json(result)),
        Err(error) if error.to_string().starts_with("未找到可重试来源") => {
            Err(ApiError::not_found(error.to_string()))
        }
        Err(error) => Err(error.into()),
    }
}

#[endpoint(tags("admin"), summary = "忽略单个失败来源")]
async fn ignore_failed_source(
    path: FeishuSourcePath,
    depot: &mut Depot,
) -> ApiResult<serde_json::Value> {
    let state = state_from_depot(depot)?;
    if !state
        .workflow
        .data_import_repo
        .ignore_failed_source(path.feishu_source_id)
        .await?
    {
        return Err(ApiError::not_found(format!(
            "未找到可忽略来源：{}",
            path.feishu_source_id
        )));
    }
    state.query_cache.invalidate_all_shared().await?;
    Ok(Json(
        serde_json::json!({ "ok": true, "feishu_source_id": path.feishu_source_id }),
    ))
}

#[endpoint(tags("admin"), summary = "查询数据异常隔离区")]
async fn list_quarantine(
    query: OperationalListQuery,
    depot: &mut Depot,
) -> ApiResult<Vec<QuarantineDto>> {
    let state = state_from_depot(depot)?;
    let rows = sqlx::query(
        r#"
        SELECT quarantine_id, feishu_source_id, content_config_id,
            content_type::text AS content_type, unique_key, reason_code,
            reason_message, created_at, resolved_at
        FROM xingtu_data_quarantine
        ORDER BY resolved_at NULLS FIRST, created_at DESC
        LIMIT $1 OFFSET $2
        "#,
    )
    .bind(query.limit.unwrap_or(50).clamp(1, 500))
    .bind(query.offset.unwrap_or(0).max(0))
    .fetch_all(&state.pool)
    .await?;
    Ok(Json(
        rows.into_iter()
            .map(|row| {
                Ok(QuarantineDto {
                    quarantine_id: row.try_get("quarantine_id")?,
                    feishu_source_id: row.try_get("feishu_source_id")?,
                    content_config_id: row.try_get("content_config_id")?,
                    content_type: row.try_get("content_type")?,
                    unique_key: row.try_get("unique_key")?,
                    reason_code: row.try_get("reason_code")?,
                    reason_message: row.try_get("reason_message")?,
                    created_at: row.try_get("created_at")?,
                    resolved_at: row.try_get("resolved_at")?,
                })
            })
            .collect::<Result<Vec<_>, sqlx::Error>>()?,
    ))
}

#[endpoint(
    tags("admin"),
    summary = "查询机器人所在的群聊",
    description = "以应用身份调用飞书群列表接口，并透传飞书官方 code/data/msg 响应和 HTTP 状态码。"
)]
async fn list_bot_chats(
    query: FeishuChatListQuery,
    depot: &mut Depot,
    res: &mut Response,
) -> ApiResult<serde_json::Value> {
    let query = build_chat_list_query(query)?;
    let state = state_from_depot(depot)?;
    let upstream = state
        .workflow
        .lark
        .get_openapi_json(&["im", "v1", "chats"], &query)
        .await
        .map_err(ApiError::bad_gateway)?;
    set_upstream_status(res, upstream.status);
    Ok(Json(upstream.body))
}

#[endpoint(
    tags("admin"),
    summary = "根据飞书多维表链接枚举数据表",
    description = "从 wiki/base 链接提取 app_token，以应用身份请求飞书数据表列表；page_size 固定为 99，并透传飞书官方 code/data/msg 响应。"
)]
async fn list_bitable_tables(
    query: FeishuBitableTablesQuery,
    depot: &mut Depot,
    res: &mut Response,
) -> ApiResult<serde_json::Value> {
    let app_token = parse_bitable_app_token(&query.url)?;
    let mut params = vec![("page_size", "99".to_owned())];
    append_feishu_page_token(&mut params, query.page_token)?;
    let state = state_from_depot(depot)?;
    let upstream = state
        .workflow
        .lark
        .get_openapi_json(&["bitable", "v1", "apps", &app_token, "tables"], &params)
        .await
        .map_err(ApiError::bad_gateway)?;
    set_upstream_status(res, upstream.status);
    Ok(Json(upstream.body))
}

#[endpoint(
    tags("admin"),
    summary = "查询指定群聊的成员",
    description = "以应用身份调用飞书群成员接口；member_id_type 支持 open_id、union_id、user_id，并透传飞书官方响应。"
)]
async fn list_chat_members(
    path: FeishuChatPath,
    query: FeishuChatMembersQuery,
    depot: &mut Depot,
    res: &mut Response,
) -> ApiResult<serde_json::Value> {
    let chat_id = validate_feishu_chat_id(&path.chat_id)?;
    let query = build_chat_members_query(query)?;
    let state = state_from_depot(depot)?;
    let upstream = state
        .workflow
        .lark
        .get_openapi_json(&["im", "v1", "chats", &chat_id, "members"], &query)
        .await
        .map_err(ApiError::bad_gateway)?;
    set_upstream_status(res, upstream.status);
    Ok(Json(upstream.body))
}

fn build_chat_list_query(
    query: FeishuChatListQuery,
) -> Result<Vec<(&'static str, String)>, ApiError> {
    let mut params = Vec::with_capacity(4);
    if let Some(value) = query.user_id_type {
        params.push((
            "user_id_type",
            validate_feishu_id_type(&value, "user_id_type")?,
        ));
    }
    if let Some(value) = query.sort_type {
        let value = value.trim();
        if !matches!(value, "ByCreateTimeAsc" | "ByActiveTimeDesc") {
            return Err(ApiError::bad_request(
                "sort_type 只支持 ByCreateTimeAsc 或 ByActiveTimeDesc",
            ));
        }
        params.push(("sort_type", value.to_owned()));
    }
    append_feishu_pagination(&mut params, query.page_size, query.page_token)?;
    Ok(params)
}

fn build_chat_members_query(
    query: FeishuChatMembersQuery,
) -> Result<Vec<(&'static str, String)>, ApiError> {
    let mut params = Vec::with_capacity(3);
    if let Some(value) = query.member_id_type {
        params.push((
            "member_id_type",
            validate_feishu_id_type(&value, "member_id_type")?,
        ));
    }
    append_feishu_pagination(&mut params, query.page_size, query.page_token)?;
    Ok(params)
}

fn append_feishu_pagination(
    params: &mut Vec<(&'static str, String)>,
    page_size: Option<i32>,
    page_token: Option<String>,
) -> Result<(), ApiError> {
    if let Some(page_size) = page_size {
        if !(1..=100).contains(&page_size) {
            return Err(ApiError::bad_request("page_size 必须在 1 到 100 之间"));
        }
        params.push(("page_size", page_size.to_string()));
    }
    append_feishu_page_token(params, page_token)
}

fn append_feishu_page_token(
    params: &mut Vec<(&'static str, String)>,
    page_token: Option<String>,
) -> Result<(), ApiError> {
    if let Some(page_token) = page_token {
        let page_token = page_token.trim();
        if page_token.is_empty() {
            return Err(ApiError::bad_request("page_token 不能为空"));
        }
        if page_token.len() > 4096 {
            return Err(ApiError::bad_request("page_token 过长"));
        }
        params.push(("page_token", page_token.to_owned()));
    }
    Ok(())
}

fn parse_bitable_app_token(input: &str) -> Result<String, ApiError> {
    let input = input.trim().trim_matches(['"', '\'']);
    let value = if input.starts_with('[') {
        input
            .rsplit_once("](")
            .and_then(|(_, target)| target.strip_suffix(')'))
            .unwrap_or(input)
    } else {
        input
    };
    let url = url::Url::parse(value.trim())
        .map_err(|_| ApiError::bad_request("url 必须是合法的飞书 HTTPS 链接"))?;
    if url.scheme() != "https" {
        return Err(ApiError::bad_request("url 必须使用 HTTPS"));
    }
    let host = url.host_str().unwrap_or_default().to_ascii_lowercase();
    if !(host == "feishu.cn"
        || host.ends_with(".feishu.cn")
        || host == "larksuite.com"
        || host.ends_with(".larksuite.com"))
    {
        return Err(ApiError::bad_request(
            "url 域名必须属于 feishu.cn 或 larksuite.com",
        ));
    }
    let segments = url
        .path_segments()
        .map(|segments| {
            segments
                .filter(|value| !value.is_empty())
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    if segments.len() != 2 || !matches!(segments[0], "wiki" | "base") {
        return Err(ApiError::bad_request(
            "url 路径必须是 /wiki/{app_token} 或 /base/{app_token}",
        ));
    }
    let token = segments[1].trim();
    if token.is_empty()
        || token.len() > 128
        || !token
            .bytes()
            .all(|value| value.is_ascii_alphanumeric() || matches!(value, b'_' | b'-'))
    {
        return Err(ApiError::bad_request("链接中的 app_token 格式不合法"));
    }
    Ok(token.to_owned())
}

fn validate_feishu_id_type(value: &str, field: &str) -> Result<String, ApiError> {
    let value = value.trim();
    if matches!(value, "open_id" | "union_id" | "user_id") {
        Ok(value.to_owned())
    } else {
        Err(ApiError::bad_request(format!(
            "{field} 只支持 open_id、union_id 或 user_id"
        )))
    }
}

fn validate_feishu_chat_id(value: &str) -> Result<String, ApiError> {
    let value = value.trim();
    if value.len() <= 128
        && value.starts_with("oc_")
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
    {
        Ok(value.to_owned())
    } else {
        Err(ApiError::bad_request("chat_id 必须是有效的飞书群聊 ID"))
    }
}

fn set_upstream_status(res: &mut Response, status: u16) {
    res.status_code(StatusCode::from_u16(status).unwrap_or(StatusCode::BAD_GATEWAY));
}

#[endpoint(tags("admin"), summary = "查询系统业务健康度")]
async fn system_status(depot: &mut Depot) -> ApiResult<SystemStatusDto> {
    let state = state_from_depot(depot)?;
    let row = sqlx::query(
        r#"
        SELECT
            (SELECT COUNT(*) FROM workflow_run WHERE status = 'running') AS running_workflows,
            (SELECT COUNT(*) FROM xingtu_feishu_source WHERE is_imported = false AND import_status = 'failed' AND ignored_at IS NULL) AS failed_sources,
            (SELECT COUNT(*) FROM xingtu_feishu_source WHERE dead_letter_at IS NOT NULL AND ignored_at IS NULL) AS dead_letter_sources,
            (SELECT COUNT(*) FROM xingtu_data_quarantine WHERE resolved_at IS NULL) AS unresolved_quarantine_rows,
            (SELECT COUNT(*) FROM xingtu_project_account WHERE is_active = true AND session_status = 'invalid') AS invalid_accounts,
            (SELECT MAX(finished_at) FROM workflow_run WHERE status = 'succeeded') AS latest_success_at,
            (SELECT MAX(finished_at) FROM workflow_run WHERE status = 'failed') AS latest_failure_at
        "#,
    )
    .fetch_one(&state.pool)
    .await?;
    Ok(Json(SystemStatusDto {
        running_workflows: row.try_get("running_workflows")?,
        failed_sources: row.try_get("failed_sources")?,
        dead_letter_sources: row.try_get("dead_letter_sources")?,
        unresolved_quarantine_rows: row.try_get("unresolved_quarantine_rows")?,
        invalid_accounts: row.try_get("invalid_accounts")?,
        latest_success_at: row.try_get("latest_success_at")?,
        latest_failure_at: row.try_get("latest_failure_at")?,
    }))
}

#[endpoint(tags("admin"), summary = "查询各项目运行与数据新鲜度")]
async fn project_statuses(depot: &mut Depot) -> ApiResult<Vec<ProjectStatusDto>> {
    let state = state_from_depot(depot)?;
    let rows = sqlx::query(
        r#"
        SELECT a.activity_period_id, project.project_key AS project, a.period,
            account.session_status::text AS account_status,
            (
                SELECT MAX(step.finished_at)
                FROM workflow_step step
                WHERE step.activity_period_id = a.activity_period_id
                    AND step.status = 'succeeded'
                    AND step.step_name IN ('table_sync', 'manual_sync', 'pending_import', 'audit_result_sync')
            ) AS latest_success_at,
            (
                SELECT MAX(step.finished_at)
                FROM workflow_step step
                WHERE step.activity_period_id = a.activity_period_id
                    AND step.status = 'failed'
            ) AS latest_failure_at,
            GREATEST(
                (SELECT MAX(v.last_seen_at) FROM video_content v
                    JOIN xingtu_activity_content_config c ON c.content_config_id = v.content_config_id
                    WHERE c.activity_period_id = a.activity_period_id),
                (SELECT MAX(l.last_seen_at) FROM live_session l
                    JOIN xingtu_activity_content_config c ON c.content_config_id = l.content_config_id
                    WHERE c.activity_period_id = a.activity_period_id)
            ) AS latest_data_at,
            (SELECT COUNT(*) FROM xingtu_feishu_source s
                JOIN xingtu_activity_content_config c ON c.content_config_id = s.content_config_id
                WHERE c.activity_period_id = a.activity_period_id AND s.is_imported = false
                    AND s.import_status = 'pending') AS pending_sources,
            (SELECT COUNT(*) FROM xingtu_feishu_source s
                JOIN xingtu_activity_content_config c ON c.content_config_id = s.content_config_id
                WHERE c.activity_period_id = a.activity_period_id AND s.is_imported = false
                    AND s.import_status = 'failed' AND s.ignored_at IS NULL) AS failed_sources,
            (SELECT COUNT(*) FROM xingtu_data_quarantine q
                JOIN xingtu_activity_content_config c ON c.content_config_id = q.content_config_id
                WHERE c.activity_period_id = a.activity_period_id AND q.resolved_at IS NULL) AS quarantine_rows
        FROM xingtu_activity_period a
        JOIN xingtu_project project ON project.project_id = a.project_id
        LEFT JOIN xingtu_project_account account
            ON account.xingtu_account_id = a.xingtu_account_id
        WHERE a.is_active = true
        ORDER BY a.task_month DESC, a.activity_period_id DESC
        "#,
    )
    .fetch_all(&state.pool)
    .await?;
    Ok(Json(
        rows.into_iter()
            .map(|row| {
                Ok(ProjectStatusDto {
                    activity_period_id: row.try_get("activity_period_id")?,
                    project: row.try_get("project")?,
                    period: row.try_get("period")?,
                    account_status: row.try_get("account_status")?,
                    latest_success_at: row.try_get("latest_success_at")?,
                    latest_failure_at: row.try_get("latest_failure_at")?,
                    latest_data_at: row.try_get("latest_data_at")?,
                    pending_sources: row.try_get("pending_sources")?,
                    failed_sources: row.try_get("failed_sources")?,
                    quarantine_rows: row.try_get("quarantine_rows")?,
                })
            })
            .collect::<Result<Vec<_>, sqlx::Error>>()?,
    ))
}

async fn fetch_activity_detail(
    pool: &PgPool,
    project_id: i64,
    activity_period_id: i64,
) -> Result<ActivityDetailDto, ApiError> {
    let activity_row = sqlx::query(&format!(
        "{} WHERE period.project_id = $1 AND period.activity_period_id = $2",
        activity_select_sql()
    ))
    .bind(project_id)
    .bind(activity_period_id)
    .fetch_optional(pool)
    .await?
    .ok_or_else(|| ApiError::not_found(format!("活动不存在：{activity_period_id}")))?;

    let content_rows = sqlx::query(
        r#"
        SELECT
            content_config_id,
            content_type::text AS content_type,
            xingtu_task_id,
            xingtu_task_name,
            source_spreadsheet_url,
            source_spreadsheet_url_update_mode::text AS source_spreadsheet_url_update_mode,
            manual_table_id,
            main_table_id,
            audit_table_id,
            data_source_field,
            spreadsheet_source_value,
            manual_source_value,
            manual_overrides_spreadsheet,
            manual_auto_approve,
            manual_auto_approve_result,
            sync_enabled,
            trace_enabled,
            remark,
            created_at,
            updated_at
        FROM xingtu_activity_content_config
        WHERE activity_period_id = $1
        ORDER BY content_type
        "#,
    )
    .bind(activity_period_id)
    .fetch_all(pool)
    .await?;

    Ok(ActivityDetailDto {
        activity: activity_from_row(activity_row)?,
        contents: content_rows
            .into_iter()
            .map(activity_content_from_row)
            .collect::<Result<Vec<_>, _>>()?,
    })
}

fn master_project_select_sql() -> &'static str {
    r#"
    SELECT
        project.project_id,
        project.project_key,
        project.display_name,
        project.is_active,
        project.remark,
        (SELECT COUNT(*) FROM xingtu_project_account account WHERE account.project_id = project.project_id) AS account_count,
        (SELECT COUNT(*) FROM xingtu_project_auditor auditor WHERE auditor.project_id = project.project_id) AS auditor_count,
        (SELECT COUNT(*) FROM xingtu_activity_period period WHERE period.project_id = project.project_id) AS period_count,
        project.created_at,
        project.updated_at
    FROM xingtu_project project
    WHERE
        ($1::bigint IS NULL OR project.project_id = $1)
        AND ($2::text IS NULL OR project.project_key = $2)
        AND ($3::boolean OR project.is_active = true)
    ORDER BY project.project_key, project.project_id
    "#
}

async fn fetch_master_project_detail(
    pool: &PgPool,
    project_id: i64,
) -> Result<MasterProjectDetailDto, ApiError> {
    let project_row = sqlx::query(master_project_select_sql())
        .bind(project_id)
        .bind(Option::<String>::None)
        .bind(true)
        .fetch_optional(pool)
        .await?
        .ok_or_else(|| ApiError::not_found(format!("主项目不存在：{project_id}")))?;
    let account_rows = sqlx::query(&format!(
        "{} ORDER BY is_default DESC, is_active DESC, xingtu_account_id",
        project_account_select_sql()
    ))
    .bind(project_id)
    .bind(true)
    .fetch_all(pool)
    .await?;
    let auditor_rows = sqlx::query(
        r#"
        SELECT project_auditor_id, project_id, auditor_name, auditor_id,
            auditor_id_type::text AS auditor_id_type, sort_order, is_active, created_at, updated_at
        FROM xingtu_project_auditor
        WHERE project_id = $1
        ORDER BY sort_order, project_auditor_id
        "#,
    )
    .bind(project_id)
    .fetch_all(pool)
    .await?;
    let period_rows = sqlx::query(&format!(
        "{} WHERE period.project_id = $1 ORDER BY period.task_month DESC, period.activity_period_id DESC",
        activity_select_sql()
    ))
    .bind(project_id)
    .fetch_all(pool)
    .await?;

    Ok(MasterProjectDetailDto {
        project: master_project_from_row(project_row)?,
        notification: fetch_project_notification(pool, project_id).await?,
        accounts: account_rows
            .into_iter()
            .map(master_project_account_from_row)
            .collect::<Result<Vec<_>, _>>()?,
        auditors: auditor_rows
            .into_iter()
            .map(project_auditor_from_row)
            .collect::<Result<Vec<_>, _>>()?,
        periods: period_rows
            .into_iter()
            .map(activity_from_row)
            .collect::<Result<Vec<_>, _>>()?,
    })
}

fn master_project_from_row(row: PgRow) -> Result<MasterProjectDto, sqlx::Error> {
    Ok(MasterProjectDto {
        project_id: row.try_get("project_id")?,
        project_key: row.try_get("project_key")?,
        display_name: row.try_get("display_name")?,
        is_active: row.try_get("is_active")?,
        remark: row.try_get("remark")?,
        account_count: row.try_get("account_count")?,
        auditor_count: row.try_get("auditor_count")?,
        period_count: row.try_get("period_count")?,
        created_at: row.try_get("created_at")?,
        updated_at: row.try_get("updated_at")?,
    })
}

fn master_project_account_from_row(row: PgRow) -> Result<MasterProjectAccountDto, sqlx::Error> {
    Ok(MasterProjectAccountDto {
        xingtu_account_id: row.try_get("xingtu_account_id")?,
        project_id: row.try_get("project_id")?,
        display_name: row.try_get("display_name")?,
        ops_ids: row.try_get("ops_ids")?,
        login_check_enabled: row.try_get("login_check_enabled")?,
        session_status: row.try_get("session_status")?,
        is_default: row.try_get("is_default")?,
        is_active: row.try_get("is_active")?,
        remark: row.try_get("remark")?,
    })
}

fn project_account_select_sql() -> &'static str {
    r#"
    SELECT xingtu_account_id, project_id, display_name, ops_ids,
        login_check_enabled, session_status::text AS session_status,
        is_default, is_active, remark
    FROM xingtu_project_account
    WHERE project_id = $1 AND ($2::boolean OR is_active = true)
    "#
}

fn project_auditor_select_sql() -> &'static str {
    r#"
    SELECT project_auditor_id, project_id, auditor_name, auditor_id,
        auditor_id_type::text AS auditor_id_type, sort_order, is_active, created_at, updated_at
    FROM xingtu_project_auditor
    WHERE project_id = $1 AND ($2::boolean OR is_active = true)
    ORDER BY sort_order, project_auditor_id
    "#
}

async fn fetch_project_notification(
    pool: &PgPool,
    project_id: i64,
) -> Result<ProjectNotificationDto, ApiError> {
    let row = sqlx::query(
        r#"
        SELECT project_id, notification_receive_id_type::text AS notification_receive_id_type,
            notification_receive_id, audit_notice_card_template_id,
            audit_result_field, login_notice_card_template_id
        FROM xingtu_project WHERE project_id = $1
        "#,
    )
    .bind(project_id)
    .fetch_optional(pool)
    .await?
    .ok_or_else(|| ApiError::not_found(format!("主项目不存在：{project_id}")))?;
    Ok(ProjectNotificationDto {
        project_id: row.try_get("project_id")?,
        notification_receive_id_type: row.try_get("notification_receive_id_type")?,
        notification_receive_id: row.try_get("notification_receive_id")?,
        audit_notice_card_template_id: row.try_get("audit_notice_card_template_id")?,
        audit_result_field: row.try_get("audit_result_field")?,
        login_notice_card_template_id: row.try_get("login_notice_card_template_id")?,
    })
}

async fn fetch_project_account(
    pool: &PgPool,
    project_id: i64,
    account_id: &str,
) -> Result<MasterProjectAccountDto, ApiError> {
    let row = sqlx::query(
        r#"
        SELECT xingtu_account_id, project_id, display_name, ops_ids,
            login_check_enabled, session_status::text AS session_status,
            is_default, is_active, remark
        FROM xingtu_project_account
        WHERE project_id = $1 AND xingtu_account_id = $2
        "#,
    )
    .bind(project_id)
    .bind(account_id)
    .fetch_optional(pool)
    .await?
    .ok_or_else(|| ApiError::not_found("该项目下不存在指定星图账号"))?;
    Ok(master_project_account_from_row(row)?)
}

async fn fetch_project_auditor(
    pool: &PgPool,
    project_id: i64,
    auditor_id: i64,
) -> Result<ProjectAuditorDto, ApiError> {
    let row = sqlx::query(
        r#"
        SELECT project_auditor_id, project_id, auditor_name, auditor_id,
            auditor_id_type::text AS auditor_id_type, sort_order, is_active, created_at, updated_at
        FROM xingtu_project_auditor
        WHERE project_id = $1 AND project_auditor_id = $2
        "#,
    )
    .bind(project_id)
    .bind(auditor_id)
    .fetch_optional(pool)
    .await?
    .ok_or_else(|| ApiError::not_found("该项目下不存在指定审核员"))?;
    Ok(project_auditor_from_row(row)?)
}

fn activity_select_sql() -> String {
    format!(
        "SELECT {} FROM xingtu_activity_period period JOIN xingtu_project project ON project.project_id = period.project_id",
        activity_columns_sql()
    )
}

fn activity_columns_sql() -> &'static str {
    r#"
        period.activity_period_id,
        period.project_id,
        project.project_key,
        project.display_name AS project_display_name,
        period.period,
        period.period_code,
        period.xingtu_account_id,
        period.task_month,
        period.bitable_url,
        period.cpm_table_id,
        period.need_trace,
        period.morning_review_enabled,
        period.periodic_sync_enabled,
        period.periodic_sync_interval_hours,
        period.tracking_start_date,
        period.tracking_end_date,
        period.is_active,
        period.remark,
        period.created_at,
        period.updated_at
    "#
}

fn project_auditor_from_row(row: PgRow) -> Result<ProjectAuditorDto, sqlx::Error> {
    Ok(ProjectAuditorDto {
        project_auditor_id: row.try_get("project_auditor_id")?,
        project_id: row.try_get("project_id")?,
        auditor_name: row.try_get("auditor_name")?,
        auditor_id: row.try_get("auditor_id")?,
        auditor_id_type: row.try_get("auditor_id_type")?,
        sort_order: row.try_get("sort_order")?,
        is_active: row.try_get("is_active")?,
        created_at: row.try_get("created_at")?,
        updated_at: row.try_get("updated_at")?,
    })
}

fn activity_from_row(row: PgRow) -> Result<ActivityAdminDto, sqlx::Error> {
    Ok(ActivityAdminDto {
        activity_period_id: row.try_get("activity_period_id")?,
        project_id: row.try_get("project_id")?,
        project_key: row.try_get("project_key")?,
        project_display_name: row.try_get("project_display_name")?,
        period: row.try_get("period")?,
        period_code: row.try_get("period_code")?,
        xingtu_account_id: row.try_get("xingtu_account_id")?,
        task_month: row.try_get("task_month")?,
        bitable_url: row.try_get("bitable_url")?,
        cpm_table_id: row.try_get("cpm_table_id")?,
        need_trace: row.try_get("need_trace")?,
        morning_review_enabled: row.try_get("morning_review_enabled")?,
        periodic_sync_enabled: row.try_get("periodic_sync_enabled")?,
        periodic_sync_interval_hours: row.try_get("periodic_sync_interval_hours")?,
        tracking_start_date: row.try_get("tracking_start_date")?,
        tracking_end_date: row.try_get("tracking_end_date")?,
        is_active: row.try_get("is_active")?,
        remark: row.try_get("remark")?,
        created_at: row.try_get("created_at")?,
        updated_at: row.try_get("updated_at")?,
    })
}

fn activity_content_from_row(row: PgRow) -> Result<ActivityContentAdminDto, sqlx::Error> {
    Ok(ActivityContentAdminDto {
        content_config_id: row.try_get("content_config_id")?,
        content_type: row.try_get("content_type")?,
        xingtu_task_id: row.try_get("xingtu_task_id")?,
        xingtu_task_name: row.try_get("xingtu_task_name")?,
        source_spreadsheet_url: row.try_get("source_spreadsheet_url")?,
        source_spreadsheet_url_update_mode: row.try_get("source_spreadsheet_url_update_mode")?,
        manual_table_id: row.try_get("manual_table_id")?,
        main_table_id: row.try_get("main_table_id")?,
        audit_table_id: row.try_get("audit_table_id")?,
        data_source_field: row.try_get("data_source_field")?,
        spreadsheet_source_value: row.try_get("spreadsheet_source_value")?,
        manual_source_value: row.try_get("manual_source_value")?,
        manual_overrides_spreadsheet: row.try_get("manual_overrides_spreadsheet")?,
        manual_auto_approve: row.try_get("manual_auto_approve")?,
        manual_auto_approve_result: row.try_get("manual_auto_approve_result")?,
        sync_enabled: row.try_get("sync_enabled")?,
        trace_enabled: row.try_get("trace_enabled")?,
        remark: row.try_get("remark")?,
        created_at: row.try_get("created_at")?,
        updated_at: row.try_get("updated_at")?,
    })
}

fn validate_receive_id_type(value: &str) -> Result<(), ApiError> {
    parse_receive_id_type(value)
        .map(|_| ())
        .map_err(|error| ApiError::bad_request(error.to_string()))
}

fn validate_auditor_id_type(auditor_id: &str, auditor_id_type: &str) -> Result<(), ApiError> {
    let auditor_id = auditor_id.trim();
    let auditor_id_type = auditor_id_type.trim();
    match auditor_id_type {
        "open_id" if auditor_id.starts_with("ou_") => Ok(()),
        "union_id" if auditor_id.starts_with("on_") => Ok(()),
        "user_id"
            if !auditor_id.starts_with("ou_")
                && !auditor_id.starts_with("on_")
                && !auditor_id.starts_with("oc_")
                && !auditor_id.contains('@') =>
        {
            Ok(())
        }
        "open_id" => Err(ApiError::bad_request(
            "auditor_id_type=open_id 时 auditor_id 必须以 ou_ 开头",
        )),
        "union_id" => Err(ApiError::bad_request(
            "auditor_id_type=union_id 时 auditor_id 必须以 on_ 开头",
        )),
        "user_id" => Err(ApiError::bad_request(
            "auditor_id 看起来不是 user_id；ou_ 请使用 open_id，on_ 请使用 union_id",
        )),
        _ => Err(ApiError::bad_request(
            "auditor_id_type 仅支持 open_id、user_id、union_id",
        )),
    }
}

fn map_project_auditor_insert_error(error: sqlx::Error) -> ApiError {
    let constraint = error
        .as_database_error()
        .and_then(|database_error| database_error.constraint());
    match constraint {
        Some("uq_xingtu_project_auditor_project_auditor")
        | Some("xingtu_project_auditor_project_auditor_id_key") => {
            ApiError::conflict("该项目已存在相同 auditor_id 的审核员")
        }
        Some("xingtu_project_auditor_pkey") => ApiError::internal_with_message(
            "审核员编号序列异常，请联系管理员校正数据库序列后重试",
            error,
        ),
        _ => error.into(),
    }
}

fn validate_project_notification(body: &ProjectNotificationInput) -> Result<(), ApiError> {
    validate_receive_id_type(&body.notification_receive_id_type)?;
    required_text(&body.notification_receive_id, "notification_receive_id")?;
    required_text(
        &body.audit_notice_card_template_id,
        "audit_notice_card_template_id",
    )?;
    required_text(&body.audit_result_field, "audit_result_field")?;
    required_text(
        &body.login_notice_card_template_id,
        "login_notice_card_template_id",
    )?;
    Ok(())
}

fn validate_project_period_request(body: &UpsertProjectPeriodRequest) -> Result<(), ApiError> {
    required_text(&body.period, "period")?;
    if Datelike::day(&body.task_month) != 1 {
        return Err(ApiError::bad_request(
            "task_month 必须是对应月份的第一天（YYYY-MM-01）",
        ));
    }
    let bitable_url = required_text(&body.bitable_url, "bitable_url")?;
    let url = url::Url::parse(&bitable_url)
        .map_err(|_| ApiError::bad_request("bitable_url 不是合法 URL"))?;
    if url.scheme() != "https" || url.host_str().is_none() {
        return Err(ApiError::bad_request(
            "bitable_url 必须是带域名的 HTTPS URL",
        ));
    }
    if body.workflows.periodic_sync_interval_hours <= 0 {
        return Err(ApiError::bad_request(
            "periodic_sync_interval_hours 必须大于 0",
        ));
    }
    if body
        .tracking_start_date
        .zip(body.tracking_end_date)
        .is_some_and(|(start, end)| start > end)
    {
        return Err(ApiError::bad_request(
            "tracking_start_date 不能晚于 tracking_end_date",
        ));
    }
    validate_activity_contents(&body.contents)
        .map_err(|error| ApiError::bad_request(error.to_string()))
}

async fn ensure_project(pool: &PgPool, project_id: i64) -> Result<(), ApiError> {
    let exists: bool =
        sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM xingtu_project WHERE project_id = $1)")
            .bind(project_id)
            .fetch_one(pool)
            .await?;
    if exists {
        Ok(())
    } else {
        Err(ApiError::not_found(format!("主项目不存在：{project_id}")))
    }
}

async fn lock_project(
    tx: &mut Transaction<'_, Postgres>,
    project_id: i64,
    require_active: bool,
) -> Result<(), ApiError> {
    let active: Option<bool> =
        sqlx::query_scalar("SELECT is_active FROM xingtu_project WHERE project_id = $1 FOR UPDATE")
            .bind(project_id)
            .fetch_optional(&mut **tx)
            .await?;
    match active {
        None => Err(ApiError::not_found(format!("主项目不存在：{project_id}"))),
        Some(false) if require_active => Err(ApiError::conflict("主项目已停用")),
        Some(_) => Ok(()),
    }
}

async fn resolve_project_account(
    tx: &mut Transaction<'_, Postgres>,
    project_id: i64,
    requested: Option<&str>,
) -> Result<String, ApiError> {
    let requested = requested.map(str::trim).filter(|value| !value.is_empty());
    let account: Option<String> = if let Some(account_id) = requested {
        sqlx::query_scalar(
            "SELECT xingtu_account_id FROM xingtu_project_account WHERE project_id = $1 AND xingtu_account_id = $2 AND is_active = true",
        )
        .bind(project_id)
        .bind(account_id)
        .fetch_optional(&mut **tx)
        .await?
    } else {
        sqlx::query_scalar(
            "SELECT xingtu_account_id FROM xingtu_project_account WHERE project_id = $1 AND is_active = true ORDER BY is_default DESC, created_at, xingtu_account_id LIMIT 1",
        )
        .bind(project_id)
        .fetch_optional(&mut **tx)
        .await?
    };
    account.ok_or_else(|| ApiError::conflict("项目没有可用的星图账号"))
}

fn normalize_string_list(values: Vec<String>) -> Vec<String> {
    let mut values = values
        .into_iter()
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
        .collect::<Vec<_>>();
    values.sort();
    values.dedup();
    values
}

fn required_optional_text(value: Option<String>, field: &str) -> Result<Option<String>, ApiError> {
    value.map(|value| required_text(&value, field)).transpose()
}

fn validate_auditor_update(body: &UpdateAuditorRequest) -> Result<(), ApiError> {
    if body.auditor_name.is_none()
        && body.auditor_id_type.is_none()
        && body.sort_order.is_none()
        && body.is_active.is_none()
    {
        return Err(ApiError::bad_request("至少提供一个需要更新的审核人字段"));
    }
    Ok(())
}

fn validate_activity_status_update(body: &UpdateActivityStatusRequest) -> Result<(), ApiError> {
    if body.is_active.is_none()
        && body.need_trace.is_none()
        && body.morning_review_enabled.is_none()
        && body.periodic_sync_enabled.is_none()
    {
        return Err(ApiError::bad_request("至少提供一个需要更新的活动状态字段"));
    }
    Ok(())
}

fn validate_card_message_date_range(
    date_from: Option<NaiveDate>,
    date_to: Option<NaiveDate>,
) -> Result<(), ApiError> {
    if date_from.zip(date_to).is_some_and(|(from, to)| from > to) {
        return Err(ApiError::bad_request("date_from 不能晚于 date_to"));
    }
    Ok(())
}

fn required_text(value: &str, field: &str) -> Result<String, ApiError> {
    let value = value.trim();
    if value.is_empty() {
        Err(ApiError::bad_request(format!("{field} 不能为空")))
    } else {
        Ok(value.to_string())
    }
}

fn trimmed_optional(value: Option<String>) -> Option<String> {
    value
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn default_user_id_type() -> String {
    "user_id".to_string()
}

fn default_true() -> bool {
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_activity_status_update_is_rejected() {
        let body = UpdateActivityStatusRequest {
            is_active: None,
            need_trace: None,
            morning_review_enabled: None,
            periodic_sync_enabled: None,
        };
        assert!(validate_activity_status_update(&body).is_err());
    }

    #[test]
    fn auditor_id_type_must_match_feishu_id_prefix() {
        assert!(validate_auditor_id_type("ou_example", "open_id").is_ok());
        assert!(validate_auditor_id_type("on_example", "union_id").is_ok());
        assert!(validate_auditor_id_type("deadbeef", "user_id").is_ok());

        assert!(validate_auditor_id_type("ou_example", "user_id").is_err());
        assert!(validate_auditor_id_type("on_example", "open_id").is_err());
        assert!(validate_auditor_id_type("oc_example", "user_id").is_err());
        assert!(validate_auditor_id_type("user@example.com", "email").is_err());
    }

    #[test]
    fn card_message_history_rejects_reversed_date_range() {
        let date_from = NaiveDate::from_ymd_opt(2026, 8, 7);
        let date_to = NaiveDate::from_ymd_opt(2026, 8, 6);

        assert!(validate_card_message_date_range(date_from, date_to).is_err());
        assert!(validate_card_message_date_range(date_to, date_from).is_ok());
    }

    #[test]
    fn card_message_endpoints_are_in_openapi() {
        let openapi =
            salvo::oapi::OpenApi::new("test", "1.0.0").merge_router_with_base(&routes(), "/api/v1");

        assert!(openapi.paths.contains_key("/api/v1/admin/card-messages"));
        assert!(
            openapi
                .paths
                .contains_key("/api/v1/admin/card-messages/{message_id}/recall")
        );
        for path in [
            "/api/v1/admin/status",
            "/api/v1/admin/periods/statuses",
            "/api/v1/admin/workflow-runs",
            "/api/v1/admin/failed-sources",
            "/api/v1/admin/quarantine",
            "/api/v1/admin/xingtu/session-upload-token",
            "/api/v1/admin/feishu/spreadsheets/format-analysis",
            "/api/v1/admin/feishu/chats",
            "/api/v1/admin/feishu/chats/{chat_id}/members",
            "/api/v1/admin/feishu/bitable/tables",
            "/api/v1/admin/projects",
            "/api/v1/admin/projects/{project_id}",
            "/api/v1/admin/projects/{project_id}/notification",
            "/api/v1/admin/projects/{project_id}/accounts",
            "/api/v1/admin/projects/{project_id}/accounts/{xingtu_account_id}",
            "/api/v1/admin/projects/{project_id}/auditors",
            "/api/v1/admin/projects/{project_id}/auditors/{project_auditor_id}",
            "/api/v1/admin/projects/{project_id}/periods",
            "/api/v1/admin/projects/{project_id}/periods/{activity_period_id}",
            "/api/v1/admin/projects/{project_id}/periods/{activity_period_id}/status",
        ] {
            assert!(openapi.paths.contains_key(path), "missing {path}");
        }
        for removed in [
            "/api/v1/admin/activities",
            "/api/v1/admin/activities/validate",
            "/api/v1/admin/auditors",
            "/api/v1/admin/projects/status",
        ] {
            assert!(
                !openapi.paths.contains_key(removed),
                "old path remains: {removed}"
            );
        }
    }

    #[tokio::test]
    async fn xingtu_extension_token_response_is_never_cacheable() {
        use salvo::http::header::{CACHE_CONTROL, PRAGMA};
        use salvo::test::{ResponseExt, TestClient};

        #[handler]
        async fn probe(res: &mut Response) -> ApiResult<XingtuExtensionConfigDto> {
            build_xingtu_extension_config(Some("test-session-upload-token".to_owned()), res)
        }

        let mut response = TestClient::get("http://127.0.0.1/")
            .send(&Service::new(Router::new().get(probe)))
            .await;
        assert_eq!(response.status_code, Some(StatusCode::OK));
        assert_eq!(
            response.headers().get(CACHE_CONTROL).unwrap(),
            "no-store, private"
        );
        assert_eq!(response.headers().get(PRAGMA).unwrap(), "no-cache");

        let body = response.take_json::<serde_json::Value>().await.unwrap();
        assert_eq!(
            body["xingtu_session_upload_token"],
            "test-session-upload-token"
        );
        assert_eq!(body["upload_path"], "/api/v1/xingtu/sessions");
        assert_eq!(body["token_type"], "Bearer");
    }

    #[tokio::test]
    async fn xingtu_extension_token_requires_dedicated_server_config() {
        use salvo::test::{ResponseExt, TestClient};

        #[handler]
        async fn probe(res: &mut Response) -> ApiResult<XingtuExtensionConfigDto> {
            build_xingtu_extension_config(None, res)
        }

        let mut response = TestClient::get("http://127.0.0.1/")
            .send(&Service::new(Router::new().get(probe)))
            .await;
        assert_eq!(response.status_code, Some(StatusCode::SERVICE_UNAVAILABLE));
        assert_eq!(
            response.headers().get(CACHE_CONTROL).unwrap(),
            "no-store, private"
        );
        assert_eq!(response.headers().get(PRAGMA).unwrap(), "no-cache");
        let body = response.take_json::<serde_json::Value>().await.unwrap();
        assert_eq!(body["code"], "service_unavailable");
        assert!(
            body["message"]
                .as_str()
                .is_some_and(|message| message.contains("XINGTU_SESSION_UPLOAD_TOKEN"))
        );
    }

    #[test]
    fn xingtu_extension_token_openapi_never_contains_a_secret_value() {
        let openapi =
            salvo::oapi::OpenApi::new("test", "1.0.0").merge_router_with_base(&routes(), "/api/v1");
        let document = serde_json::to_string(&openapi).unwrap();
        assert!(document.contains("/api/v1/admin/xingtu/session-upload-token"));
        assert!(!document.contains("test-session-upload-token"));
    }

    #[test]
    fn feishu_chat_list_query_only_accepts_official_values() {
        let params = build_chat_list_query(FeishuChatListQuery {
            user_id_type: Some("union_id".to_owned()),
            sort_type: Some("ByActiveTimeDesc".to_owned()),
            page_size: Some(100),
            page_token: Some("next-token".to_owned()),
        })
        .unwrap();
        assert_eq!(
            params,
            vec![
                ("user_id_type", "union_id".to_owned()),
                ("sort_type", "ByActiveTimeDesc".to_owned()),
                ("page_size", "100".to_owned()),
                ("page_token", "next-token".to_owned()),
            ]
        );

        assert!(
            build_chat_list_query(FeishuChatListQuery {
                user_id_type: Some("chat_id".to_owned()),
                ..Default::default()
            })
            .is_err()
        );
        assert!(
            build_chat_list_query(FeishuChatListQuery {
                sort_type: Some("unknown".to_owned()),
                ..Default::default()
            })
            .is_err()
        );
        assert!(
            build_chat_list_query(FeishuChatListQuery {
                page_size: Some(101),
                ..Default::default()
            })
            .is_err()
        );
    }

    #[test]
    fn feishu_chat_member_query_supports_user_and_union_ids() {
        for member_id_type in ["open_id", "user_id", "union_id"] {
            let params = build_chat_members_query(FeishuChatMembersQuery {
                member_id_type: Some(member_id_type.to_owned()),
                page_size: Some(50),
                page_token: None,
            })
            .unwrap();
            assert_eq!(params[0], ("member_id_type", member_id_type.to_owned()));
        }
        assert!(validate_feishu_chat_id("oc_xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx").is_ok());
        assert!(validate_feishu_chat_id("../../admin").is_err());
    }

    #[test]
    fn bitable_app_token_is_extracted_from_plain_and_markdown_links() {
        let expected = "ExampleBitableToken";
        let plain = format!(
            "https://example.feishu.cn/wiki/{expected}?table=tblExampleDataTable&view=vew3grghtl"
        );
        let markdown = format!("[{plain}]({plain})");
        assert_eq!(parse_bitable_app_token(&plain).unwrap(), expected);
        assert_eq!(parse_bitable_app_token(&markdown).unwrap(), expected);
        assert_eq!(
            parse_bitable_app_token(&format!(
                "https://example.larksuite.com/base/{expected}?table=tbl123"
            ))
            .unwrap(),
            expected
        );
    }

    #[test]
    fn bitable_app_token_rejects_untrusted_or_malformed_links() {
        assert!(parse_bitable_app_token("http://aa.feishu.cn/wiki/token").is_err());
        assert!(parse_bitable_app_token("https://evil.example/wiki/token").is_err());
        assert!(parse_bitable_app_token("https://aa.feishu.cn/wiki/").is_err());
        assert!(parse_bitable_app_token("https://aa.feishu.cn/docx/token").is_err());
    }

    #[tokio::test]
    async fn admin_console_is_served_without_embedding_a_token() {
        use salvo::test::{ResponseExt, TestClient};

        let mut response = TestClient::get("http://127.0.0.1/admin-console")
            .send(&Service::new(ui_routes()))
            .await;
        assert_eq!(response.status_code, Some(StatusCode::OK));
        let html = response.take_string().await.unwrap();
        assert!(html.contains("内部业务控制台"));
        assert!(!html.contains("Bearer ey"));
    }
}
