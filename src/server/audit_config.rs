use crate::lark::im::parse_receive_id_type;
use crate::server::api::RequiredJsonBody;
use crate::server::auth::require_default_actor;
use crate::server::error::{ApiError, ApiResult};
use crate::server::state::state_from_depot;
use chrono::{DateTime, Utc};
use salvo::oapi::{ToParameters, ToSchema};
use salvo::prelude::*;
use serde::{Deserialize, Serialize};
use sqlx::{PgPool, Row};

pub fn routes() -> Router {
    Router::new()
        .push(
            Router::with_path("audit-configs")
                .hoop(require_default_actor)
                .get(list_audit_configs)
                .post(create_audit_config),
        )
        .push(
            Router::with_path("audit-configs/{audit_config_id}")
                .hoop(require_default_actor)
                .get(get_audit_config)
                .put(update_audit_config)
                .delete(delete_audit_config),
        )
        .push(
            Router::with_path("audit-configs/{audit_config_id}/targets")
                .hoop(require_default_actor)
                .post(create_notification_target),
        )
        .push(
            Router::with_path(
                "audit-configs/{audit_config_id}/targets/{audit_notification_target_id}",
            )
            .hoop(require_default_actor)
            .put(update_notification_target)
            .delete(delete_notification_target),
        )
        .push(
            Router::with_path(
                "audit-configs/{audit_config_id}/targets/{audit_notification_target_id}/auditors",
            )
            .hoop(require_default_actor)
            .post(create_target_auditor),
        )
        .push(
            Router::with_path(
                "audit-configs/{audit_config_id}/targets/{audit_notification_target_id}/auditors/{audit_notification_auditor_id}",
            )
            .hoop(require_default_actor)
            .put(update_target_auditor)
            .delete(delete_target_auditor),
        )
        .push(
            Router::with_path("projects/{project_id}/audit-config-binding")
                .hoop(require_default_actor)
                .get(get_project_binding)
                .put(bind_project)
                .delete(unbind_project),
        )
}

#[derive(Debug, Deserialize, ToParameters)]
#[salvo(parameters(default_parameter_in = Path))]
struct AuditConfigPath {
    audit_config_id: i64,
}

#[derive(Debug, Deserialize, ToParameters)]
#[salvo(parameters(default_parameter_in = Path))]
struct NotificationTargetPath {
    audit_config_id: i64,
    audit_notification_target_id: i64,
}

#[derive(Debug, Deserialize, ToParameters)]
#[salvo(parameters(default_parameter_in = Path))]
struct TargetAuditorPath {
    audit_config_id: i64,
    audit_notification_target_id: i64,
    audit_notification_auditor_id: i64,
}

#[derive(Debug, Deserialize, ToParameters)]
#[salvo(parameters(default_parameter_in = Path))]
struct ProjectPath {
    project_id: i64,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct AuditConfigDto {
    pub audit_config_id: i64,
    pub config_name: String,
    pub feishu_app_id: Option<i64>,
    pub feishu_app_name: Option<String>,
    pub card_template_id: String,
    pub audit_result_field: String,
    pub is_active: bool,
    pub remark: Option<String>,
    pub project_count: i64,
    pub targets: Vec<AuditNotificationTargetDto>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct AuditNotificationTargetDto {
    pub audit_notification_target_id: i64,
    pub audit_config_id: i64,
    pub target_name: String,
    pub receive_id_type: String,
    pub receive_id: String,
    pub sort_order: i32,
    pub is_active: bool,
    pub auditors: Vec<AuditNotificationAuditorDto>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct AuditNotificationAuditorDto {
    pub audit_notification_auditor_id: i64,
    pub audit_notification_target_id: i64,
    pub auditor_name: String,
    pub auditor_id: String,
    pub auditor_id_type: String,
    pub sort_order: i32,
    pub is_active: bool,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct ProjectAuditConfigBindingDto {
    pub project_id: i64,
    pub audit_config_id: i64,
    pub audit_config_name: String,
    pub audit_notification_target_id: i64,
    pub target_name: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
struct CreateAuditConfigRequest {
    config_name: String,
    feishu_app_id: i64,
    card_template_id: String,
    #[serde(default = "default_audit_result_field")]
    audit_result_field: String,
    #[serde(default = "default_true")]
    is_active: bool,
    remark: Option<String>,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
struct UpdateAuditConfigRequest {
    config_name: String,
    feishu_app_id: i64,
    card_template_id: String,
    audit_result_field: String,
    #[serde(default = "default_true")]
    is_active: bool,
    remark: Option<String>,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
struct SaveNotificationTargetRequest {
    target_name: String,
    #[serde(default = "default_chat_id_type")]
    receive_id_type: String,
    receive_id: String,
    #[serde(default)]
    sort_order: i32,
    #[serde(default = "default_true")]
    is_active: bool,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
struct SaveTargetAuditorRequest {
    auditor_name: String,
    auditor_id: String,
    #[serde(default = "default_user_id_type")]
    auditor_id_type: String,
    #[serde(default)]
    sort_order: i32,
    #[serde(default = "default_true")]
    is_active: bool,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
struct BindProjectRequest {
    audit_config_id: i64,
    audit_notification_target_id: i64,
}

#[endpoint(tags("admin"), summary = "查询共享审核配置")]
async fn list_audit_configs(depot: &mut Depot) -> ApiResult<Vec<AuditConfigDto>> {
    let state = state_from_depot(depot)?;
    Ok(Json(fetch_audit_configs(&state.pool, None).await?))
}

#[endpoint(tags("admin"), summary = "查询共享审核配置详情")]
async fn get_audit_config(path: AuditConfigPath, depot: &mut Depot) -> ApiResult<AuditConfigDto> {
    let state = state_from_depot(depot)?;
    Ok(Json(
        fetch_audit_config(&state.pool, path.audit_config_id).await?,
    ))
}

#[endpoint(tags("admin"), summary = "创建共享审核配置")]
async fn create_audit_config(
    body: RequiredJsonBody<CreateAuditConfigRequest>,
    depot: &mut Depot,
) -> ApiResult<AuditConfigDto> {
    let state = state_from_depot(depot)?;
    let body = body.into_inner();
    validate_audit_config_input(
        &state.pool,
        &body.config_name,
        body.feishu_app_id,
        &body.card_template_id,
        &body.audit_result_field,
    )
    .await?;
    let audit_config_id: i64 = sqlx::query_scalar(
        r#"
        INSERT INTO xingtu_audit_config (
            config_name, feishu_app_id, card_template_id, audit_result_field, is_active, remark
        ) VALUES ($1, $2, $3, $4, $5, $6)
        RETURNING audit_config_id
        "#,
    )
    .bind(body.config_name.trim())
    .bind(body.feishu_app_id)
    .bind(body.card_template_id.trim())
    .bind(body.audit_result_field.trim())
    .bind(body.is_active)
    .bind(trimmed_optional(body.remark))
    .fetch_one(&state.pool)
    .await
    .map_err(map_write_error)?;
    state.query_cache.invalidate_all_shared().await?;
    Ok(Json(
        fetch_audit_config(&state.pool, audit_config_id).await?,
    ))
}

#[endpoint(tags("admin"), summary = "更新共享审核配置")]
async fn update_audit_config(
    path: AuditConfigPath,
    body: RequiredJsonBody<UpdateAuditConfigRequest>,
    depot: &mut Depot,
) -> ApiResult<AuditConfigDto> {
    let state = state_from_depot(depot)?;
    let body = body.into_inner();
    validate_audit_config_input(
        &state.pool,
        &body.config_name,
        body.feishu_app_id,
        &body.card_template_id,
        &body.audit_result_field,
    )
    .await?;
    let result = sqlx::query(
        r#"
        UPDATE xingtu_audit_config SET
            config_name = $2, feishu_app_id = $3, card_template_id = $4,
            audit_result_field = $5, is_active = $6, remark = $7
        WHERE audit_config_id = $1
        "#,
    )
    .bind(path.audit_config_id)
    .bind(body.config_name.trim())
    .bind(body.feishu_app_id)
    .bind(body.card_template_id.trim())
    .bind(body.audit_result_field.trim())
    .bind(body.is_active)
    .bind(trimmed_optional(body.remark))
    .execute(&state.pool)
    .await
    .map_err(map_write_error)?;
    if result.rows_affected() == 0 {
        return Err(ApiError::not_found("审核配置不存在"));
    }
    state.query_cache.invalidate_all_shared().await?;
    Ok(Json(
        fetch_audit_config(&state.pool, path.audit_config_id).await?,
    ))
}

#[endpoint(tags("admin"), summary = "删除未绑定的共享审核配置")]
async fn delete_audit_config(path: AuditConfigPath, depot: &mut Depot) -> ApiResult<()> {
    let state = state_from_depot(depot)?;
    let result = sqlx::query("DELETE FROM xingtu_audit_config WHERE audit_config_id = $1")
        .bind(path.audit_config_id)
        .execute(&state.pool)
        .await
        .map_err(map_write_error)?;
    if result.rows_affected() == 0 {
        return Err(ApiError::not_found("审核配置不存在"));
    }
    state.query_cache.invalidate_all_shared().await?;
    Ok(Json(()))
}

#[endpoint(tags("admin"), summary = "新增审核通知方案")]
async fn create_notification_target(
    path: AuditConfigPath,
    body: RequiredJsonBody<SaveNotificationTargetRequest>,
    depot: &mut Depot,
) -> ApiResult<AuditNotificationTargetDto> {
    let state = state_from_depot(depot)?;
    ensure_audit_config_exists(&state.pool, path.audit_config_id).await?;
    let body = body.into_inner();
    validate_target_input(&body)?;
    let target_id: i64 = sqlx::query_scalar(
        r#"
        INSERT INTO xingtu_audit_notification_target (
            audit_config_id, target_name, receive_id_type, receive_id, sort_order, is_active
        ) VALUES ($1, $2, $3::xingtu_receive_id_type, $4, $5, $6)
        RETURNING audit_notification_target_id
        "#,
    )
    .bind(path.audit_config_id)
    .bind(body.target_name.trim())
    .bind(body.receive_id_type.trim())
    .bind(body.receive_id.trim())
    .bind(body.sort_order)
    .bind(body.is_active)
    .fetch_one(&state.pool)
    .await
    .map_err(map_write_error)?;
    state.query_cache.invalidate_all_shared().await?;
    Ok(Json(
        fetch_target(&state.pool, path.audit_config_id, target_id).await?,
    ))
}

#[endpoint(tags("admin"), summary = "更新审核通知方案")]
async fn update_notification_target(
    path: NotificationTargetPath,
    body: RequiredJsonBody<SaveNotificationTargetRequest>,
    depot: &mut Depot,
) -> ApiResult<AuditNotificationTargetDto> {
    let state = state_from_depot(depot)?;
    let body = body.into_inner();
    validate_target_input(&body)?;
    let result = sqlx::query(
        r#"
        UPDATE xingtu_audit_notification_target SET
            target_name = $3, receive_id_type = $4::xingtu_receive_id_type,
            receive_id = $5, sort_order = $6, is_active = $7
        WHERE audit_config_id = $1 AND audit_notification_target_id = $2
        "#,
    )
    .bind(path.audit_config_id)
    .bind(path.audit_notification_target_id)
    .bind(body.target_name.trim())
    .bind(body.receive_id_type.trim())
    .bind(body.receive_id.trim())
    .bind(body.sort_order)
    .bind(body.is_active)
    .execute(&state.pool)
    .await
    .map_err(map_write_error)?;
    if result.rows_affected() == 0 {
        return Err(ApiError::not_found("审核通知方案不存在"));
    }
    state.query_cache.invalidate_all_shared().await?;
    Ok(Json(
        fetch_target(
            &state.pool,
            path.audit_config_id,
            path.audit_notification_target_id,
        )
        .await?,
    ))
}

#[endpoint(tags("admin"), summary = "删除未绑定的审核通知方案")]
async fn delete_notification_target(
    path: NotificationTargetPath,
    depot: &mut Depot,
) -> ApiResult<()> {
    let state = state_from_depot(depot)?;
    let result = sqlx::query(
        "DELETE FROM xingtu_audit_notification_target WHERE audit_config_id = $1 AND audit_notification_target_id = $2",
    )
    .bind(path.audit_config_id)
    .bind(path.audit_notification_target_id)
    .execute(&state.pool)
    .await
    .map_err(map_write_error)?;
    if result.rows_affected() == 0 {
        return Err(ApiError::not_found("审核通知方案不存在"));
    }
    state.query_cache.invalidate_all_shared().await?;
    Ok(Json(()))
}

#[endpoint(tags("admin"), summary = "新增通知方案审核员")]
async fn create_target_auditor(
    path: NotificationTargetPath,
    body: RequiredJsonBody<SaveTargetAuditorRequest>,
    depot: &mut Depot,
) -> ApiResult<AuditNotificationAuditorDto> {
    let state = state_from_depot(depot)?;
    ensure_target_exists(
        &state.pool,
        path.audit_config_id,
        path.audit_notification_target_id,
    )
    .await?;
    let body = body.into_inner();
    validate_auditor_input(&body)?;
    let auditor_id: i64 = sqlx::query_scalar(
        r#"
        INSERT INTO xingtu_audit_notification_auditor (
            audit_notification_target_id, auditor_name, auditor_id,
            auditor_id_type, sort_order, is_active
        ) VALUES ($1, $2, $3, $4::xingtu_receive_id_type, $5, $6)
        RETURNING audit_notification_auditor_id
        "#,
    )
    .bind(path.audit_notification_target_id)
    .bind(body.auditor_name.trim())
    .bind(body.auditor_id.trim())
    .bind(body.auditor_id_type.trim())
    .bind(body.sort_order)
    .bind(body.is_active)
    .fetch_one(&state.pool)
    .await
    .map_err(map_write_error)?;
    state.query_cache.invalidate_all_shared().await?;
    Ok(Json(
        fetch_auditor(&state.pool, path.audit_notification_target_id, auditor_id).await?,
    ))
}

#[endpoint(tags("admin"), summary = "更新通知方案审核员")]
async fn update_target_auditor(
    path: TargetAuditorPath,
    body: RequiredJsonBody<SaveTargetAuditorRequest>,
    depot: &mut Depot,
) -> ApiResult<AuditNotificationAuditorDto> {
    let state = state_from_depot(depot)?;
    ensure_target_exists(
        &state.pool,
        path.audit_config_id,
        path.audit_notification_target_id,
    )
    .await?;
    let body = body.into_inner();
    validate_auditor_input(&body)?;
    let result = sqlx::query(
        r#"
        UPDATE xingtu_audit_notification_auditor SET
            auditor_name = $3, auditor_id = $4,
            auditor_id_type = $5::xingtu_receive_id_type,
            sort_order = $6, is_active = $7
        WHERE audit_notification_target_id = $1 AND audit_notification_auditor_id = $2
        "#,
    )
    .bind(path.audit_notification_target_id)
    .bind(path.audit_notification_auditor_id)
    .bind(body.auditor_name.trim())
    .bind(body.auditor_id.trim())
    .bind(body.auditor_id_type.trim())
    .bind(body.sort_order)
    .bind(body.is_active)
    .execute(&state.pool)
    .await
    .map_err(map_write_error)?;
    if result.rows_affected() == 0 {
        return Err(ApiError::not_found("审核员不存在"));
    }
    state.query_cache.invalidate_all_shared().await?;
    Ok(Json(
        fetch_auditor(
            &state.pool,
            path.audit_notification_target_id,
            path.audit_notification_auditor_id,
        )
        .await?,
    ))
}

#[endpoint(tags("admin"), summary = "删除通知方案审核员")]
async fn delete_target_auditor(path: TargetAuditorPath, depot: &mut Depot) -> ApiResult<()> {
    let state = state_from_depot(depot)?;
    ensure_target_exists(
        &state.pool,
        path.audit_config_id,
        path.audit_notification_target_id,
    )
    .await?;
    let result = sqlx::query(
        "DELETE FROM xingtu_audit_notification_auditor WHERE audit_notification_target_id = $1 AND audit_notification_auditor_id = $2",
    )
    .bind(path.audit_notification_target_id)
    .bind(path.audit_notification_auditor_id)
    .execute(&state.pool)
    .await?;
    if result.rows_affected() == 0 {
        return Err(ApiError::not_found("审核员不存在"));
    }
    state.query_cache.invalidate_all_shared().await?;
    Ok(Json(()))
}

#[endpoint(tags("admin"), summary = "查询项目审核配置绑定")]
async fn get_project_binding(
    path: ProjectPath,
    depot: &mut Depot,
) -> ApiResult<Option<ProjectAuditConfigBindingDto>> {
    let state = state_from_depot(depot)?;
    ensure_project_exists(&state.pool, path.project_id).await?;
    Ok(Json(
        fetch_project_binding(&state.pool, path.project_id).await?,
    ))
}

#[endpoint(tags("admin"), summary = "绑定项目审核配置及通知方案")]
async fn bind_project(
    path: ProjectPath,
    body: RequiredJsonBody<BindProjectRequest>,
    depot: &mut Depot,
) -> ApiResult<ProjectAuditConfigBindingDto> {
    let state = state_from_depot(depot)?;
    let body = body.into_inner();
    ensure_project_exists(&state.pool, path.project_id).await?;
    let valid: bool = sqlx::query_scalar(
        r#"
        SELECT EXISTS (
            SELECT 1
            FROM xingtu_audit_config config
            JOIN xingtu_audit_notification_target target
              ON target.audit_config_id = config.audit_config_id
            JOIN xingtu_feishu_app app ON app.feishu_app_id = config.feishu_app_id
            WHERE config.audit_config_id = $1
              AND target.audit_notification_target_id = $2
              AND config.is_active = true AND target.is_active = true AND app.is_active = true
        )
        "#,
    )
    .bind(body.audit_config_id)
    .bind(body.audit_notification_target_id)
    .fetch_one(&state.pool)
    .await?;
    if !valid {
        return Err(ApiError::conflict(
            "审核配置、通知方案或飞书应用不存在/未启用，或通知方案不属于该配置",
        ));
    }
    sqlx::query(
        r#"
        INSERT INTO xingtu_project_audit_config_binding (
            project_id, audit_config_id, audit_notification_target_id
        ) VALUES ($1, $2, $3)
        ON CONFLICT (project_id) DO UPDATE SET
            audit_config_id = EXCLUDED.audit_config_id,
            audit_notification_target_id = EXCLUDED.audit_notification_target_id
        "#,
    )
    .bind(path.project_id)
    .bind(body.audit_config_id)
    .bind(body.audit_notification_target_id)
    .execute(&state.pool)
    .await
    .map_err(map_write_error)?;
    state.query_cache.invalidate_all_shared().await?;
    Ok(Json(
        fetch_project_binding(&state.pool, path.project_id)
            .await?
            .ok_or_else(|| ApiError::internal("项目审核配置绑定后未能读取"))?,
    ))
}

#[endpoint(tags("admin"), summary = "解除项目审核配置绑定")]
async fn unbind_project(path: ProjectPath, depot: &mut Depot) -> ApiResult<()> {
    let state = state_from_depot(depot)?;
    let result =
        sqlx::query("DELETE FROM xingtu_project_audit_config_binding WHERE project_id = $1")
            .bind(path.project_id)
            .execute(&state.pool)
            .await?;
    if result.rows_affected() == 0 {
        return Err(ApiError::not_found("项目尚未绑定审核配置"));
    }
    state.query_cache.invalidate_all_shared().await?;
    Ok(Json(()))
}

async fn fetch_audit_configs(
    pool: &PgPool,
    audit_config_id: Option<i64>,
) -> Result<Vec<AuditConfigDto>, ApiError> {
    let rows = sqlx::query(
        r#"
        SELECT config.audit_config_id, config.config_name, config.feishu_app_id,
            app.display_name AS feishu_app_name, config.card_template_id,
            config.audit_result_field, config.is_active, config.remark,
            config.created_at, config.updated_at,
            count(DISTINCT binding.project_id) AS project_count
        FROM xingtu_audit_config config
        LEFT JOIN xingtu_feishu_app app ON app.feishu_app_id = config.feishu_app_id
        LEFT JOIN xingtu_project_audit_config_binding binding
          ON binding.audit_config_id = config.audit_config_id
        WHERE ($1::bigint IS NULL OR config.audit_config_id = $1)
        GROUP BY config.audit_config_id, app.display_name
        ORDER BY config.config_name, config.audit_config_id
        "#,
    )
    .bind(audit_config_id)
    .fetch_all(pool)
    .await?;
    let mut configs = Vec::with_capacity(rows.len());
    for row in rows {
        let config_id: i64 = row.try_get("audit_config_id")?;
        configs.push(AuditConfigDto {
            audit_config_id: config_id,
            config_name: row.try_get("config_name")?,
            feishu_app_id: row.try_get("feishu_app_id")?,
            feishu_app_name: row.try_get("feishu_app_name")?,
            card_template_id: row.try_get("card_template_id")?,
            audit_result_field: row.try_get("audit_result_field")?,
            is_active: row.try_get("is_active")?,
            remark: row.try_get("remark")?,
            project_count: row.try_get("project_count")?,
            targets: fetch_targets(pool, config_id).await?,
            created_at: row.try_get("created_at")?,
            updated_at: row.try_get("updated_at")?,
        });
    }
    Ok(configs)
}

async fn fetch_audit_config(
    pool: &PgPool,
    audit_config_id: i64,
) -> Result<AuditConfigDto, ApiError> {
    fetch_audit_configs(pool, Some(audit_config_id))
        .await?
        .into_iter()
        .next()
        .ok_or_else(|| ApiError::not_found("审核配置不存在"))
}

async fn fetch_targets(
    pool: &PgPool,
    audit_config_id: i64,
) -> Result<Vec<AuditNotificationTargetDto>, ApiError> {
    let rows = sqlx::query(
        r#"
        SELECT audit_notification_target_id, audit_config_id, target_name,
            receive_id_type::text AS receive_id_type, receive_id,
            sort_order, is_active, created_at, updated_at
        FROM xingtu_audit_notification_target
        WHERE audit_config_id = $1
        ORDER BY sort_order, audit_notification_target_id
        "#,
    )
    .bind(audit_config_id)
    .fetch_all(pool)
    .await?;
    let mut targets = Vec::with_capacity(rows.len());
    for row in rows {
        let target_id: i64 = row.try_get("audit_notification_target_id")?;
        targets.push(AuditNotificationTargetDto {
            audit_notification_target_id: target_id,
            audit_config_id: row.try_get("audit_config_id")?,
            target_name: row.try_get("target_name")?,
            receive_id_type: row.try_get("receive_id_type")?,
            receive_id: row.try_get("receive_id")?,
            sort_order: row.try_get("sort_order")?,
            is_active: row.try_get("is_active")?,
            auditors: fetch_auditors(pool, target_id).await?,
            created_at: row.try_get("created_at")?,
            updated_at: row.try_get("updated_at")?,
        });
    }
    Ok(targets)
}

async fn fetch_target(
    pool: &PgPool,
    audit_config_id: i64,
    target_id: i64,
) -> Result<AuditNotificationTargetDto, ApiError> {
    fetch_targets(pool, audit_config_id)
        .await?
        .into_iter()
        .find(|target| target.audit_notification_target_id == target_id)
        .ok_or_else(|| ApiError::not_found("审核通知方案不存在"))
}

async fn fetch_auditors(
    pool: &PgPool,
    target_id: i64,
) -> Result<Vec<AuditNotificationAuditorDto>, ApiError> {
    let rows = sqlx::query(
        r#"
        SELECT audit_notification_auditor_id, audit_notification_target_id,
            auditor_name, auditor_id, auditor_id_type::text AS auditor_id_type,
            sort_order, is_active, created_at, updated_at
        FROM xingtu_audit_notification_auditor
        WHERE audit_notification_target_id = $1
        ORDER BY sort_order, audit_notification_auditor_id
        "#,
    )
    .bind(target_id)
    .fetch_all(pool)
    .await?;
    rows.into_iter()
        .map(|row| {
            Ok(AuditNotificationAuditorDto {
                audit_notification_auditor_id: row.try_get("audit_notification_auditor_id")?,
                audit_notification_target_id: row.try_get("audit_notification_target_id")?,
                auditor_name: row.try_get("auditor_name")?,
                auditor_id: row.try_get("auditor_id")?,
                auditor_id_type: row.try_get("auditor_id_type")?,
                sort_order: row.try_get("sort_order")?,
                is_active: row.try_get("is_active")?,
                created_at: row.try_get("created_at")?,
                updated_at: row.try_get("updated_at")?,
            })
        })
        .collect::<Result<Vec<_>, sqlx::Error>>()
        .map_err(Into::into)
}

async fn fetch_auditor(
    pool: &PgPool,
    target_id: i64,
    auditor_id: i64,
) -> Result<AuditNotificationAuditorDto, ApiError> {
    fetch_auditors(pool, target_id)
        .await?
        .into_iter()
        .find(|auditor| auditor.audit_notification_auditor_id == auditor_id)
        .ok_or_else(|| ApiError::not_found("审核员不存在"))
}

pub(crate) async fn fetch_project_binding(
    pool: &PgPool,
    project_id: i64,
) -> Result<Option<ProjectAuditConfigBindingDto>, ApiError> {
    let row = sqlx::query(
        r#"
        SELECT binding.project_id, binding.audit_config_id, config.config_name,
            binding.audit_notification_target_id, target.target_name,
            binding.created_at, binding.updated_at
        FROM xingtu_project_audit_config_binding binding
        JOIN xingtu_audit_config config ON config.audit_config_id = binding.audit_config_id
        JOIN xingtu_audit_notification_target target
          ON target.audit_notification_target_id = binding.audit_notification_target_id
         AND target.audit_config_id = binding.audit_config_id
        WHERE binding.project_id = $1
        "#,
    )
    .bind(project_id)
    .fetch_optional(pool)
    .await?;
    row.map(|row| -> Result<ProjectAuditConfigBindingDto, sqlx::Error> {
        Ok(ProjectAuditConfigBindingDto {
            project_id: row.try_get("project_id")?,
            audit_config_id: row.try_get("audit_config_id")?,
            audit_config_name: row.try_get("config_name")?,
            audit_notification_target_id: row.try_get("audit_notification_target_id")?,
            target_name: row.try_get("target_name")?,
            created_at: row.try_get("created_at")?,
            updated_at: row.try_get("updated_at")?,
        })
    })
    .transpose()
    .map_err(Into::into)
}

async fn validate_audit_config_input(
    pool: &PgPool,
    config_name: &str,
    feishu_app_id: i64,
    card_template_id: &str,
    audit_result_field: &str,
) -> Result<(), ApiError> {
    required(config_name, "config_name")?;
    required(card_template_id, "card_template_id")?;
    required(audit_result_field, "audit_result_field")?;
    if feishu_app_id <= 0 {
        return Err(ApiError::bad_request("feishu_app_id 必须大于 0"));
    }
    let active: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM xingtu_feishu_app WHERE feishu_app_id = $1 AND is_active = true)",
    )
    .bind(feishu_app_id)
    .fetch_one(pool)
    .await?;
    if !active {
        return Err(ApiError::conflict("飞书应用不存在或已停用"));
    }
    Ok(())
}

fn validate_target_input(body: &SaveNotificationTargetRequest) -> Result<(), ApiError> {
    required(&body.target_name, "target_name")?;
    required(&body.receive_id, "receive_id")?;
    parse_receive_id_type(body.receive_id_type.trim())
        .map_err(|error| ApiError::bad_request(error.to_string()))?;
    Ok(())
}

fn validate_auditor_input(body: &SaveTargetAuditorRequest) -> Result<(), ApiError> {
    required(&body.auditor_name, "auditor_name")?;
    let auditor_id = required(&body.auditor_id, "auditor_id")?;
    match body.auditor_id_type.trim() {
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
        "open_id" => Err(ApiError::bad_request("open_id 必须以 ou_ 开头")),
        "union_id" => Err(ApiError::bad_request("union_id 必须以 on_ 开头")),
        "user_id" => Err(ApiError::bad_request("auditor_id 不是有效的 user_id")),
        _ => Err(ApiError::bad_request(
            "auditor_id_type 仅支持 open_id、user_id、union_id",
        )),
    }
}

async fn ensure_audit_config_exists(pool: &PgPool, audit_config_id: i64) -> Result<(), ApiError> {
    let exists: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM xingtu_audit_config WHERE audit_config_id = $1)",
    )
    .bind(audit_config_id)
    .fetch_one(pool)
    .await?;
    if exists {
        Ok(())
    } else {
        Err(ApiError::not_found("审核配置不存在"))
    }
}

async fn ensure_target_exists(
    pool: &PgPool,
    audit_config_id: i64,
    target_id: i64,
) -> Result<(), ApiError> {
    let exists: bool = sqlx::query_scalar(
        r#"
        SELECT EXISTS(
            SELECT 1 FROM xingtu_audit_notification_target
            WHERE audit_config_id = $1 AND audit_notification_target_id = $2
        )
        "#,
    )
    .bind(audit_config_id)
    .bind(target_id)
    .fetch_one(pool)
    .await?;
    if exists {
        Ok(())
    } else {
        Err(ApiError::not_found("审核通知方案不存在"))
    }
}

async fn ensure_project_exists(pool: &PgPool, project_id: i64) -> Result<(), ApiError> {
    let exists: bool =
        sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM xingtu_project WHERE project_id = $1)")
            .bind(project_id)
            .fetch_one(pool)
            .await?;
    if exists {
        Ok(())
    } else {
        Err(ApiError::not_found("主项目不存在"))
    }
}

fn required<'a>(value: &'a str, field: &str) -> Result<&'a str, ApiError> {
    let value = value.trim();
    if value.is_empty() {
        Err(ApiError::bad_request(format!("{field} 不能为空")))
    } else {
        Ok(value)
    }
}

fn trimmed_optional(value: Option<String>) -> Option<String> {
    value.and_then(|value| {
        let value = value.trim().to_owned();
        (!value.is_empty()).then_some(value)
    })
}

fn map_write_error(error: sqlx::Error) -> ApiError {
    let constraint = error
        .as_database_error()
        .and_then(|database_error| database_error.constraint());
    match constraint {
        Some("xingtu_audit_config_config_name_key") => ApiError::conflict("审核配置名称已存在"),
        Some("uq_xingtu_audit_target_receiver") => {
            ApiError::conflict("该审核配置已存在相同通知对象")
        }
        Some("uq_xingtu_audit_target_auditor") => ApiError::conflict("该通知方案已存在相同审核员"),
        Some("xingtu_project_audit_config_binding_audit_config_id_fkey")
        | Some("fk_xingtu_project_audit_binding_target") => {
            ApiError::conflict("审核配置或通知方案仍被项目绑定")
        }
        _ => error.into(),
    }
}

fn default_true() -> bool {
    true
}

fn default_audit_result_field() -> String {
    "审核结果".to_owned()
}

fn default_chat_id_type() -> String {
    "chat_id".to_owned()
}

fn default_user_id_type() -> String {
    "user_id".to_owned()
}
