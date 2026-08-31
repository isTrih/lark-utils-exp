use crate::pipeline::activity::{
    ActivityAuditNoticeConfig, ActivityAuditResultSyncConfig, ActivityAuditTableConfig,
    ActivitySyncConfig, ActivityTableDbSyncConfig, ActivityTableSyncConfig,
    ActivityTableSyncRuleConfig, ActivityTableSyncSourceMode, AuditNoticeWorkflowConfig,
};
use crate::xingtu::data_import::{PersistMergedRecordsOptions, XingtuDataImportRepository};
use anyhow::{Context, anyhow};
use chrono::{DateTime, NaiveDate, Timelike, Utc};
use chrono_tz::Asia::Shanghai;
use salvo::oapi::ToSchema;
use serde::{Deserialize, Serialize};
use sqlx::{PgPool, Postgres, Row, Transaction};
use std::{
    collections::{HashMap, HashSet},
    path::Path,
};
use tokio::fs;
use url::Url;

/// 活动配置文件，外层数组表示同一次调度可以管理多期活动。
pub type ActivityConfigFile = Vec<ActivityConfig>;

/// 单期活动配置。
///
/// 这个结构用于调试期从 JSON 导入，也可以作为后续企业后端下发配置的 DTO。
#[derive(Debug, Clone, Deserialize, Serialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct ActivityConfig {
    /// 已存在的主项目 ID。项目身份、通知、账号和审核员必须通过管理接口维护。
    pub project_id: i64,
    pub period: String,
    #[serde(default)]
    pub period_code: Option<String>,
    /// 引用主项目下已存在的星图账号；不填时使用项目默认账号。
    #[serde(default)]
    pub xingtu_account_id: Option<String>,
    pub task_month: NaiveDate,
    pub bitable_url: String,
    #[serde(default)]
    pub cpm_table_id: Option<String>,
    #[serde(default = "default_true")]
    pub need_trace: bool,
    #[serde(default)]
    pub tracking_start_date: Option<NaiveDate>,
    #[serde(default)]
    pub tracking_end_date: Option<NaiveDate>,
    #[serde(default)]
    pub workflows: WorkflowConfig,
    pub contents: Vec<ActivityContentConfig>,
}

/// 调度开关配置。
#[derive(Debug, Clone, Deserialize, Serialize, ToSchema)]
pub struct WorkflowConfig {
    #[serde(default = "default_true")]
    pub morning_workflow_enabled: bool,
    #[serde(default = "default_true")]
    pub periodic_sync_enabled: bool,
    #[serde(default = "default_periodic_sync_interval_hours")]
    pub periodic_sync_interval_hours: i32,
}

impl Default for WorkflowConfig {
    fn default() -> Self {
        Self {
            morning_workflow_enabled: true,
            periodic_sync_enabled: true,
            periodic_sync_interval_hours: default_periodic_sync_interval_hours(),
        }
    }
}

/// 单个内容类型配置，直播/视频各一份。
#[derive(Debug, Clone, Deserialize, Serialize, ToSchema)]
pub struct ActivityContentConfig {
    pub content_type: XingtuContentType,
    #[serde(default = "default_true")]
    pub sync_enabled: bool,
    #[serde(default = "default_true")]
    pub trace_enabled: bool,
    pub xingtu_task: XingtuTaskConfig,
    #[serde(default)]
    pub source: SourceConfig,
    pub tables: TableConfig,
    #[serde(default)]
    pub sync_rule: SyncRuleConfig,
}

/// 支持的星图内容类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum XingtuContentType {
    Live,
    Video,
}

impl XingtuContentType {
    pub fn as_db_value(self) -> &'static str {
        match self {
            Self::Live => "live",
            Self::Video => "video",
        }
    }
}

/// 星图任务配置。
#[derive(Debug, Clone, Deserialize, Serialize, ToSchema)]
pub struct XingtuTaskConfig {
    pub task_id: String,
    #[serde(default)]
    pub task_name: Option<String>,
}

/// 星图源 Sheet 配置。
#[derive(Debug, Clone, Deserialize, Serialize, ToSchema)]
pub struct SourceConfig {
    #[serde(default)]
    pub spreadsheet_url: Option<String>,
    #[serde(default = "default_spreadsheet_url_update_mode")]
    pub spreadsheet_url_update_mode: String,
}

impl Default for SourceConfig {
    fn default() -> Self {
        Self {
            spreadsheet_url: None,
            spreadsheet_url_update_mode: default_spreadsheet_url_update_mode(),
        }
    }
}

/// 飞书多维表配置。
#[derive(Debug, Clone, Deserialize, Serialize, ToSchema)]
pub struct TableConfig {
    pub main_table_id: String,
    #[serde(default)]
    pub manual_table_id: Option<String>,
    pub audit_table_id: String,
}

/// 同步规则配置。
#[derive(Debug, Clone, Deserialize, Serialize, ToSchema)]
pub struct SyncRuleConfig {
    #[serde(default = "default_data_source_field")]
    pub data_source_field: String,
    #[serde(default = "default_spreadsheet_source_value")]
    pub spreadsheet_source_value: String,
    #[serde(default = "default_manual_source_value")]
    pub manual_source_value: String,
    #[serde(default)]
    pub manual_overrides_spreadsheet: bool,
    #[serde(default = "default_true")]
    pub manual_auto_approve: bool,
    #[serde(default = "default_manual_auto_approve_result")]
    pub manual_auto_approve_result: String,
}

impl Default for SyncRuleConfig {
    fn default() -> Self {
        Self {
            data_source_field: default_data_source_field(),
            spreadsheet_source_value: default_spreadsheet_source_value(),
            manual_source_value: default_manual_source_value(),
            manual_overrides_spreadsheet: false,
            manual_auto_approve: true,
            manual_auto_approve_result: default_manual_auto_approve_result(),
        }
    }
}

/// 需要拉取星图源 Sheet 链接的内容配置。
///
/// `trace_enabled` 在这里仅表示“后续是否写追踪明细”，
/// 不影响拉取最新链接和同步审核表。
#[derive(Debug, Clone)]
pub struct ExportableContent {
    pub content_config_id: i64,
    pub activity_period_id: i64,
    pub project: String,
    pub period: String,
    pub xingtu_account_id: String,
    pub content_type: XingtuContentType,
    pub xingtu_task_id: String,
    pub xingtu_task_name: Option<String>,
    pub previous_spreadsheet_url: Option<String>,
    pub trace_enabled: bool,
}

pub struct FeishuSourceInsertOptions<'a> {
    pub trigger_type: &'a str,
    pub stat_date: NaiveDate,
    pub pull_started_at: DateTime<Utc>,
    pub is_daily_final: bool,
    pub created_by: &'a str,
}

/// 自动工作流中的单个活动执行范围。
#[derive(Debug, Clone)]
pub struct WorkflowActivityScope {
    pub activity_period_id: i64,
    pub xingtu_account_id: String,
}

/// 从数据库读取出来的一条同步内容配置。
#[derive(Debug, Clone)]
struct SyncableContentConfig {
    activity_period_id: i64,
    content_config_id: i64,
    period: String,
    bitable_url: String,
    content_type: XingtuContentType,
    source_spreadsheet_url: String,
    manual_table_id: Option<String>,
    main_table_id: String,
    audit_table_id: String,
    data_source_field: String,
    spreadsheet_source_value: String,
    manual_source_value: String,
    manual_overrides_spreadsheet: bool,
    manual_auto_approve: bool,
    manual_auto_approve_result: String,
    trace_enabled: bool,
    audit_result_field: String,
    latest_feishu_source_id: Option<i64>,
    latest_stat_date: Option<NaiveDate>,
    latest_is_daily_final: bool,
}

/// 活动配置数据库仓储。
#[derive(Clone)]
pub struct XingtuActivityConfigRepository {
    pool: PgPool,
}

impl XingtuActivityConfigRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// 从 JSON 文件读取活动配置。
    pub async fn load_config_file(path: impl AsRef<Path>) -> anyhow::Result<ActivityConfigFile> {
        let path = path.as_ref();
        let content = fs::read_to_string(path)
            .await
            .with_context(|| format!("读取星图活动配置失败：{}", path.display()))?;

        serde_json::from_str(&content)
            .with_context(|| format!("解析星图活动配置 JSON 失败：{}", path.display()))
    }

    /// 将配置文件 upsert 到数据库。
    pub async fn upsert_config_file(&self, configs: &[ActivityConfig]) -> anyhow::Result<()> {
        if configs.is_empty() {
            return Ok(());
        }

        let mut tx = self.pool.begin().await.context("开启数据库事务失败")?;

        for config in configs {
            self.upsert_activity_config(&mut tx, config)
                .await
                .with_context(|| format!("写入活动配置失败：{}", config.period))?;
        }

        tx.commit().await.context("提交活动配置事务失败")?;
        Ok(())
    }

    /// 查询所有需要拉取星图源 Sheet 链接的内容配置。
    ///
    /// 这里按 `sync_enabled` 过滤，而不是按 `trace_enabled` 过滤：
    /// 直播不写追踪明细，但仍然需要定时拉最新数据并更新审核表。
    pub async fn list_exportable_contents(
        &self,
        run_at: DateTime<Utc>,
        activity_period_id: Option<i64>,
    ) -> anyhow::Result<Vec<ExportableContent>> {
        let rows = sqlx::query(
            r#"
            SELECT
                c.content_config_id,
                p.activity_period_id,
                project.project_key AS project,
                p.period,
                p.xingtu_account_id,
                c.content_type::text AS content_type,
                c.xingtu_task_id,
                c.xingtu_task_name,
                c.source_spreadsheet_url,
                c.trace_enabled,
                p.tracking_start_date,
                p.tracking_end_date
            FROM xingtu_activity_content_config c
            JOIN xingtu_activity_period p
                ON p.activity_period_id = c.activity_period_id
            JOIN xingtu_project project ON project.project_id = p.project_id
            WHERE
                p.is_active = true
                AND p.need_trace = true
                AND p.periodic_sync_enabled = true
                AND c.sync_enabled = true
                AND c.source_spreadsheet_url_update_mode = 'xingtu_export'
                AND ($1::bigint IS NULL OR p.activity_period_id = $1)
            ORDER BY p.task_month, p.period, c.content_type
            "#,
        )
        .bind(activity_period_id)
        .fetch_all(&self.pool)
        .await
        .context("查询待拉取星图配置失败")?;

        let mut contents = Vec::new();
        for row in rows {
            let tracking_start_date: Option<NaiveDate> = row.try_get("tracking_start_date")?;
            let tracking_end_date: Option<NaiveDate> = row.try_get("tracking_end_date")?;
            if !tracking_window_includes(run_at, tracking_start_date, tracking_end_date) {
                continue;
            }

            let content_type: String = row.try_get("content_type")?;
            contents.push(ExportableContent {
                content_config_id: row.try_get("content_config_id")?,
                activity_period_id: row.try_get("activity_period_id")?,
                project: row.try_get("project")?,
                period: row.try_get("period")?,
                xingtu_account_id: row.try_get("xingtu_account_id")?,
                content_type: parse_content_type(&content_type)?,
                xingtu_task_id: row.try_get("xingtu_task_id")?,
                xingtu_task_name: row.try_get("xingtu_task_name")?,
                previous_spreadsheet_url: row.try_get("source_spreadsheet_url")?,
                trace_enabled: row.try_get("trace_enabled")?,
            });
        }

        Ok(contents)
    }

    /// 查询一次自动工作流需要依次处理的活动期次。
    ///
    /// 返回顺序就是执行顺序。这里只选择启用、存在同步内容且仍在追踪窗口内的期次；
    /// 传入 `activity_period_id` 时只返回指定期次。
    pub async fn list_workflow_activity_scopes(
        &self,
        run_at: DateTime<Utc>,
        activity_period_id: Option<i64>,
    ) -> anyhow::Result<Vec<WorkflowActivityScope>> {
        let rows = sqlx::query(
            r#"
            SELECT
                p.activity_period_id,
                p.xingtu_account_id,
                p.tracking_start_date,
                p.tracking_end_date
            FROM xingtu_activity_period p
            WHERE
                p.is_active = true
                AND ($1::bigint IS NULL OR p.activity_period_id = $1)
                AND EXISTS (
                    SELECT 1
                    FROM xingtu_activity_content_config c
                    WHERE
                        c.activity_period_id = p.activity_period_id
                        AND c.sync_enabled = true
                )
            ORDER BY p.task_month, p.activity_period_id
            "#,
        )
        .bind(activity_period_id)
        .fetch_all(&self.pool)
        .await
        .context("查询自动工作流活动期次失败")?;

        let mut activity_scopes = Vec::new();
        for row in rows {
            let tracking_start_date: Option<NaiveDate> = row.try_get("tracking_start_date")?;
            let tracking_end_date: Option<NaiveDate> = row.try_get("tracking_end_date")?;
            if tracking_window_includes(run_at, tracking_start_date, tracking_end_date) {
                activity_scopes.push(WorkflowActivityScope {
                    activity_period_id: row.try_get("activity_period_id")?,
                    xingtu_account_id: row.try_get("xingtu_account_id")?,
                });
            }
        }

        Ok(activity_scopes)
    }

    /// 查询可执行同步的活动配置，并组装成 pipeline 使用的 DTO。
    ///
    /// 这里面只读取已经有 `source_spreadsheet_url` 的内容配置；
    /// 星图链接拉取由 trace 流程负责，避免同步流程拿空链接执行。
    pub async fn list_sync_activity_configs(
        &self,
        data_import_repo: XingtuDataImportRepository,
        run_at: DateTime<Utc>,
        activity_period_id: Option<i64>,
    ) -> anyhow::Result<Vec<ActivitySyncConfig>> {
        self.list_activity_sync_configs(
            data_import_repo,
            ActivityTableSyncSourceMode::SpreadsheetAndManual,
            Some(run_at),
            activity_period_id,
        )
        .await
    }

    /// 查询仅包含手动登记来源的活动同步配置。
    pub async fn list_manual_sync_activity_configs(
        &self,
        data_import_repo: XingtuDataImportRepository,
        activity_period_id: Option<i64>,
    ) -> anyhow::Result<Vec<ActivitySyncConfig>> {
        self.list_activity_sync_configs(
            data_import_repo,
            ActivityTableSyncSourceMode::ManualOnly,
            None,
            activity_period_id,
        )
        .await
    }

    async fn list_activity_sync_configs(
        &self,
        data_import_repo: XingtuDataImportRepository,
        source_mode: ActivityTableSyncSourceMode,
        tracking_run_at: Option<DateTime<Utc>>,
        activity_period_id: Option<i64>,
    ) -> anyhow::Result<Vec<ActivitySyncConfig>> {
        let contents = self
            .list_syncable_contents(source_mode, tracking_run_at, activity_period_id)
            .await?;
        let mut activities: Vec<ActivitySyncConfig> = Vec::new();
        let mut period_to_index: HashMap<i64, usize> = HashMap::new();

        for content in contents {
            let index = if let Some(index) = period_to_index.get(&content.activity_period_id) {
                *index
            } else {
                let index = activities.len();
                period_to_index.insert(content.activity_period_id, index);
                activities.push(ActivitySyncConfig {
                    period: content.period.clone(),
                    live: None,
                    video: None,
                });
                index
            };

            let table_config = build_activity_table_sync_config(
                content.clone(),
                data_import_repo.clone(),
                source_mode,
            );

            match content.content_type {
                XingtuContentType::Live => activities[index].live = Some(table_config),
                XingtuContentType::Video => activities[index].video = Some(table_config),
            }
        }

        Ok(activities)
    }

    /// 查询启用活动中需要从审核表回写数据库的内容配置。
    pub async fn list_audit_result_sync_configs(
        &self,
        activity_period_id: Option<i64>,
    ) -> anyhow::Result<Vec<ActivityAuditResultSyncConfig>> {
        let rows = sqlx::query(
            r#"
            SELECT
                p.period,
                p.bitable_url,
                project.audit_result_field,
                c.content_config_id,
                c.content_type::text AS content_type,
                c.audit_table_id
            FROM xingtu_activity_content_config c
            JOIN xingtu_activity_period p
                ON p.activity_period_id = c.activity_period_id
            JOIN xingtu_project project ON project.project_id = p.project_id
            WHERE
                p.is_active = true
                AND c.sync_enabled = true
                AND btrim(c.audit_table_id) <> ''
                AND ($1::bigint IS NULL OR p.activity_period_id = $1)
            ORDER BY p.task_month, p.period, c.content_type
            "#,
        )
        .bind(activity_period_id)
        .fetch_all(&self.pool)
        .await
        .context("查询审核结果同步配置失败")?;

        rows.into_iter()
            .map(|row| {
                let content_type: String = row.try_get("content_type")?;
                Ok(ActivityAuditResultSyncConfig {
                    period: row.try_get("period")?,
                    content_config_id: row.try_get("content_config_id")?,
                    content_type: parse_content_type(&content_type)?,
                    bitable_url: row.try_get("bitable_url")?,
                    audit_table_id: row.try_get("audit_table_id")?,
                    audit_result_field: row.try_get("audit_result_field")?,
                })
            })
            .collect()
    }

    /// 从数据库按活动期次构建审核通知 workflow 配置。
    ///
    /// 每期活动的卡片跳转链接直接使用 `bitable_url`，待审核数量由通知流程读取
    /// 直播/视频 audit 表后统计 `audit_result_field` 为空的记录。每个活动期次独立
    /// 使用自己的接收群和模板，并读取所属项目当前启用的审核人。
    pub async fn build_audit_notice_configs_from_db(
        &self,
        tracking_run_at: Option<DateTime<Utc>>,
        activity_period_id: Option<i64>,
    ) -> anyhow::Result<Vec<AuditNoticeWorkflowConfig>> {
        let rows = sqlx::query(
            r#"
            SELECT
                p.activity_period_id,
                p.project_id,
                project.display_name AS project,
                p.period,
                p.bitable_url,
                project.notification_receive_id AS project_group_id,
                project.notification_receive_id_type::text AS receive_id_type,
                project.audit_notice_card_template_id AS notice_card_template_id,
                project.audit_result_field,
                p.tracking_start_date,
                p.tracking_end_date,
                c.content_type::text AS content_type,
                c.audit_table_id
            FROM xingtu_activity_period p
            JOIN xingtu_activity_content_config c
                ON c.activity_period_id = p.activity_period_id
            JOIN xingtu_project project ON project.project_id = p.project_id
            WHERE
                p.is_active = true
                AND p.morning_review_enabled = true
                AND project.is_active = true
                AND project.notification_receive_id IS NOT NULL
                AND c.sync_enabled = true
                AND btrim(c.audit_table_id) <> ''
                AND ($1::bigint IS NULL OR p.activity_period_id = $1)
            ORDER BY project.project_key, p.task_month DESC, p.activity_period_id DESC, c.content_type
            "#,
        )
        .bind(activity_period_id)
        .fetch_all(&self.pool)
        .await
        .context("查询审核通知配置失败")?;

        let mut configs = Vec::<AuditNoticeWorkflowConfig>::new();
        let mut period_to_index = HashMap::<i64, usize>::new();

        for row in rows {
            let tracking_start_date: Option<NaiveDate> = row.try_get("tracking_start_date")?;
            let tracking_end_date: Option<NaiveDate> = row.try_get("tracking_end_date")?;
            if let Some(run_at) = tracking_run_at
                && !tracking_window_includes(run_at, tracking_start_date, tracking_end_date)
            {
                continue;
            }

            let activity_period_id: i64 = row.try_get("activity_period_id")?;
            let project_id: i64 = row.try_get("project_id")?;
            let project: String = row.try_get("project")?;
            let period: String = row.try_get("period")?;
            let bitable_url: String = row.try_get("bitable_url")?;
            let project_group_id: String = row.try_get("project_group_id")?;
            let receive_id_type: String = row.try_get("receive_id_type")?;
            let card_template_id: String = row.try_get("notice_card_template_id")?;
            let audit_result_field: String = row.try_get("audit_result_field")?;
            let content_type: String = row.try_get("content_type")?;
            let audit_table_id: String = row.try_get("audit_table_id")?;

            let config_index = if let Some(index) = period_to_index.get(&activity_period_id) {
                *index
            } else {
                let index = configs.len();
                let auditor_ids = self
                    .active_auditor_ids_for_project(project_id)
                    .await
                    .with_context(|| format!("查询项目审核人失败：{project}"))?;
                configs.push(AuditNoticeWorkflowConfig {
                    receiver: crate::lark::im::MessageReceiver {
                        receive_id_type: crate::lark::im::parse_receive_id_type(&receive_id_type)?,
                        receive_id: project_group_id,
                        uuid: None,
                    },
                    card_template_id,
                    auditor_ids,
                    project_name: project.clone(),
                    audit_result_field,
                    activities: vec![ActivityAuditNoticeConfig {
                        period,
                        audit_table_url: bitable_url.clone(),
                        live: None,
                        video: None,
                    }],
                });
                period_to_index.insert(activity_period_id, index);
                index
            };

            let table_config = ActivityAuditTableConfig {
                bitable_url,
                audit_table_id,
            };

            match parse_content_type(&content_type)? {
                XingtuContentType::Live => {
                    configs[config_index].activities[0].live = Some(table_config)
                }
                XingtuContentType::Video => {
                    configs[config_index].activities[0].video = Some(table_config)
                }
            }
        }

        Ok(configs)
    }

    /// 读取项目级当前启用的审核人。
    ///
    /// 审核通知只认 `xingtu_project_auditor`；账号表中的 `ops_ids`
    /// 仅用于星图登录态和工作流错误通知。
    async fn active_auditor_ids_for_project(&self, project_id: i64) -> anyhow::Result<String> {
        let rows = sqlx::query(
            r#"
            SELECT auditor_id
            FROM xingtu_project_auditor
            WHERE
                project_id = $1
                AND is_active = true
            ORDER BY sort_order, project_auditor_id
            "#,
        )
        .bind(project_id)
        .fetch_all(&self.pool)
        .await
        .context("查询生效的项目审核人失败")?;
        let auditor_ids = rows
            .into_iter()
            .map(|row| row.try_get::<String, _>("auditor_id"))
            .collect::<Result<Vec<_>, _>>()
            .context("读取生效的项目审核人 ID 失败")?
            .join(",");

        tracing::info!(
            project_id,
            auditor_ids = %auditor_ids,
            "审核通知已读取生效的项目审核人"
        );
        Ok(auditor_ids)
    }

    async fn list_syncable_contents(
        &self,
        source_mode: ActivityTableSyncSourceMode,
        tracking_run_at: Option<DateTime<Utc>>,
        activity_period_id: Option<i64>,
    ) -> anyhow::Result<Vec<SyncableContentConfig>> {
        let manual_only = source_mode == ActivityTableSyncSourceMode::ManualOnly;
        let rows = sqlx::query(
            r#"
            SELECT
                p.activity_period_id,
                c.content_config_id,
                p.period,
                p.bitable_url,
                project.audit_result_field,
                c.content_type::text AS content_type,
                COALESCE(c.source_spreadsheet_url, '') AS source_spreadsheet_url,
                c.manual_table_id,
                c.main_table_id,
                c.audit_table_id,
                c.data_source_field,
                c.spreadsheet_source_value,
                c.manual_source_value,
                c.manual_overrides_spreadsheet,
                c.manual_auto_approve,
                c.manual_auto_approve_result,
                c.trace_enabled,
                p.tracking_start_date,
                p.tracking_end_date,
                latest_source.feishu_source_id AS latest_feishu_source_id,
                latest_source.stat_date AS latest_stat_date,
                COALESCE(latest_source.is_daily_final, false) AS latest_is_daily_final
            FROM xingtu_activity_content_config c
            JOIN xingtu_activity_period p
                ON p.activity_period_id = c.activity_period_id
            JOIN xingtu_project project ON project.project_id = p.project_id
            LEFT JOIN LATERAL (
                SELECT
                    feishu_source_id,
                    stat_date,
                    is_daily_final
                FROM xingtu_feishu_source s
                WHERE
                    s.content_config_id = c.content_config_id
                    AND s.feishu_sheet_url = c.source_spreadsheet_url
                ORDER BY s.pulled_at DESC, s.feishu_source_id DESC
                LIMIT 1
            ) latest_source ON true
            WHERE
                p.is_active = true
                AND c.sync_enabled = true
                AND (
                    (
                        $1::boolean = false
                        AND c.source_spreadsheet_url IS NOT NULL
                        AND btrim(c.source_spreadsheet_url) <> ''
                    )
                    OR (
                        $1::boolean = true
                        AND c.manual_table_id IS NOT NULL
                        AND btrim(c.manual_table_id) <> ''
                    )
                )
                AND ($2::bigint IS NULL OR p.activity_period_id = $2)
            ORDER BY p.task_month, p.period, c.content_type
            "#,
        )
        .bind(manual_only)
        .bind(activity_period_id)
        .fetch_all(&self.pool)
        .await
        .context("查询可同步活动配置失败")?;

        let mut contents = Vec::new();
        for row in rows {
            if let Some(run_at) = tracking_run_at {
                let tracking_start_date: Option<NaiveDate> = row.try_get("tracking_start_date")?;
                let tracking_end_date: Option<NaiveDate> = row.try_get("tracking_end_date")?;
                if !tracking_window_includes(run_at, tracking_start_date, tracking_end_date) {
                    continue;
                }
            }

            let content_type: String = row.try_get("content_type")?;
            contents.push(SyncableContentConfig {
                activity_period_id: row.try_get("activity_period_id")?,
                content_config_id: row.try_get("content_config_id")?,
                period: row.try_get("period")?,
                bitable_url: row.try_get("bitable_url")?,
                content_type: parse_content_type(&content_type)?,
                source_spreadsheet_url: row.try_get("source_spreadsheet_url")?,
                manual_table_id: row.try_get("manual_table_id")?,
                main_table_id: row.try_get("main_table_id")?,
                audit_table_id: row.try_get("audit_table_id")?,
                data_source_field: row.try_get("data_source_field")?,
                spreadsheet_source_value: row.try_get("spreadsheet_source_value")?,
                manual_source_value: row.try_get("manual_source_value")?,
                manual_overrides_spreadsheet: row.try_get("manual_overrides_spreadsheet")?,
                manual_auto_approve: row.try_get("manual_auto_approve")?,
                manual_auto_approve_result: row.try_get("manual_auto_approve_result")?,
                trace_enabled: row.try_get("trace_enabled")?,
                audit_result_field: row.try_get("audit_result_field")?,
                latest_feishu_source_id: row.try_get("latest_feishu_source_id")?,
                latest_stat_date: row.try_get("latest_stat_date")?,
                latest_is_daily_final: row.try_get("latest_is_daily_final")?,
            });
        }

        Ok(contents)
    }

    /// 回写最新星图导出的源 Sheet 链接。
    pub async fn update_source_spreadsheet_url(
        &self,
        content_config_id: i64,
        spreadsheet_url: &str,
    ) -> anyhow::Result<()> {
        sqlx::query(
            r#"
            UPDATE xingtu_activity_content_config
            SET
                source_spreadsheet_url = $2,
                source_spreadsheet_url_updated_at = now()
            WHERE content_config_id = $1
            "#,
        )
        .bind(content_config_id)
        .bind(spreadsheet_url)
        .execute(&self.pool)
        .await
        .with_context(|| format!("回写 source_spreadsheet_url 失败：{content_config_id}"))?;

        Ok(())
    }

    /// 记录一次星图拉取得到的飞书 Sheet 来源。
    ///
    /// Night 允许重复执行：同一内容、同一业务日期只保留一条每日最终来源，
    /// 新导出的链接覆盖该行并重新进入待导入状态。
    pub async fn insert_feishu_source(
        &self,
        content: &ExportableContent,
        spreadsheet_url: &str,
        options: FeishuSourceInsertOptions<'_>,
    ) -> anyhow::Result<i64> {
        let FeishuSourceInsertOptions {
            trigger_type,
            stat_date,
            pull_started_at,
            is_daily_final,
            created_by,
        } = options;
        let mut tx = self
            .pool
            .begin()
            .await
            .context("开启飞书来源登记事务失败")?;

        // 锁定父配置，确保多个进程同时登记同一内容来源时也按顺序执行。
        sqlx::query(
            r#"
            SELECT content_config_id
            FROM xingtu_activity_content_config
            WHERE content_config_id = $1
            FOR UPDATE
            "#,
        )
        .bind(content.content_config_id)
        .fetch_one(&mut *tx)
        .await
        .with_context(|| {
            format!(
                "锁定星图内容配置失败：content_config_id={}",
                content.content_config_id
            )
        })?;

        if is_daily_final {
            let existing = sqlx::query(
                r#"
                UPDATE xingtu_feishu_source
                SET
                    feishu_sheet_url = $3,
                    trigger_type = $4::xingtu_source_trigger_type,
                    pull_started_at = $5,
                    pulled_at = now(),
                    is_imported = false,
                    import_status = 'pending',
                    imported_row_count = 0,
                    imported_at = NULL,
                    error_message = NULL,
                    attempt_count = 0,
                    last_attempt_at = NULL,
                    next_retry_at = NULL,
                    dead_letter_at = NULL,
                    ignored_at = NULL,
                    created_by = $6
                WHERE
                    content_config_id = $1
                    AND stat_date = $2
                    AND is_daily_final = true
                RETURNING feishu_source_id
                "#,
            )
            .bind(content.content_config_id)
            .bind(stat_date)
            .bind(spreadsheet_url)
            .bind(trigger_type)
            .bind(pull_started_at)
            .bind(created_by)
            .fetch_optional(&mut *tx)
            .await
            .with_context(|| {
                format!(
                    "更新每日最终飞书来源失败：content_config_id={} stat_date={} url={spreadsheet_url}",
                    content.content_config_id, stat_date
                )
            })?;

            if let Some(row) = existing {
                let feishu_source_id: i64 = row.try_get("feishu_source_id")?;
                tx.commit().await.context("提交每日最终飞书来源更新失败")?;
                tracing::info!(
                    content_config_id = content.content_config_id,
                    %stat_date,
                    feishu_source_id,
                    url = spreadsheet_url,
                    "已复用并刷新当日最终飞书来源"
                );
                return Ok(feishu_source_id);
            }
        }

        let row = sqlx::query(
            r#"
            INSERT INTO xingtu_feishu_source (
                content_config_id,
                content_type,
                feishu_sheet_url,
                trigger_type,
                stat_date,
                pull_started_at,
                pulled_at,
                is_daily_final,
                created_by
            )
            VALUES (
                $1,
                $2::xingtu_content_type,
                $3,
                $4::xingtu_source_trigger_type,
                $5,
                $6,
                now(),
                $7,
                $8
            )
            ON CONFLICT (feishu_sheet_url)
            DO UPDATE SET
                trigger_type = EXCLUDED.trigger_type,
                stat_date = EXCLUDED.stat_date,
                pull_started_at = EXCLUDED.pull_started_at,
                pulled_at = EXCLUDED.pulled_at,
                is_daily_final = EXCLUDED.is_daily_final,
                is_imported = false,
                import_status = 'pending',
                imported_row_count = 0,
                imported_at = NULL,
                error_message = NULL,
                attempt_count = 0,
                last_attempt_at = NULL,
                next_retry_at = NULL,
                dead_letter_at = NULL,
                ignored_at = NULL,
                created_by = EXCLUDED.created_by
            WHERE
                xingtu_feishu_source.content_config_id = EXCLUDED.content_config_id
            RETURNING feishu_source_id
            "#,
        )
        .bind(content.content_config_id)
        .bind(content.content_type.as_db_value())
        .bind(spreadsheet_url)
        .bind(trigger_type)
        .bind(stat_date)
        .bind(pull_started_at)
        .bind(is_daily_final)
        .bind(created_by)
        .fetch_optional(&mut *tx)
        .await
        .with_context(|| format!("记录飞书来源失败：{spreadsheet_url}"))?
        .ok_or_else(|| {
            anyhow!(
                "飞书来源链接已属于其他内容配置：content_config_id={} url={spreadsheet_url}",
                content.content_config_id
            )
        })?;

        let feishu_source_id = row.try_get("feishu_source_id")?;
        tx.commit().await.context("提交飞书来源登记事务失败")?;
        Ok(feishu_source_id)
    }

    async fn upsert_activity_config(
        &self,
        tx: &mut Transaction<'_, Postgres>,
        config: &ActivityConfig,
    ) -> anyhow::Result<()> {
        validate_activity_config(config)?;

        let project_id = self
            .resolve_master_project(tx, config)
            .await
            .context("解析主项目配置失败")?;

        let xingtu_account_id = self
            .upsert_project_account(tx, project_id, config)
            .await
            .context("写入项目星图账号配置失败")?;
        let row = sqlx::query(
            r#"
            INSERT INTO xingtu_activity_period (
                project_id,
                period,
                period_code,
                xingtu_account_id,
                task_month,
                bitable_url,
                cpm_table_id,
                need_trace,
                morning_review_enabled,
                periodic_sync_enabled,
                periodic_sync_interval_hours,
                tracking_start_date,
                tracking_end_date
            )
            VALUES (
                $1,
                $2,
                $3,
                $4,
                $5,
                $6,
                $7,
                $8,
                $9,
                $10,
                $11,
                $12,
                $13
            )
            ON CONFLICT (project_id, period)
            DO UPDATE SET
                period_code = EXCLUDED.period_code,
                xingtu_account_id = EXCLUDED.xingtu_account_id,
                task_month = EXCLUDED.task_month,
                bitable_url = EXCLUDED.bitable_url,
                cpm_table_id = EXCLUDED.cpm_table_id,
                need_trace = EXCLUDED.need_trace,
                morning_review_enabled = EXCLUDED.morning_review_enabled,
                periodic_sync_enabled = EXCLUDED.periodic_sync_enabled,
                periodic_sync_interval_hours = EXCLUDED.periodic_sync_interval_hours,
                tracking_start_date = EXCLUDED.tracking_start_date,
                tracking_end_date = EXCLUDED.tracking_end_date
            RETURNING activity_period_id
            "#,
        )
        .bind(project_id)
        .bind(config.period.trim())
        .bind(trim_optional(config.period_code.as_deref()))
        .bind(&xingtu_account_id)
        .bind(config.task_month)
        .bind(config.bitable_url.trim())
        .bind(trim_optional(config.cpm_table_id.as_deref()))
        .bind(config.need_trace)
        .bind(config.workflows.morning_workflow_enabled)
        .bind(config.workflows.periodic_sync_enabled)
        .bind(config.workflows.periodic_sync_interval_hours)
        .bind(config.tracking_start_date)
        .bind(config.tracking_end_date)
        .fetch_one(&mut **tx)
        .await
        .context("写入活动期次失败")?;

        let activity_period_id: i64 = row.try_get("activity_period_id")?;

        self.upsert_contents(tx, activity_period_id, &config.contents)
            .await?;
        Ok(())
    }

    async fn resolve_master_project(
        &self,
        tx: &mut Transaction<'_, Postgres>,
        config: &ActivityConfig,
    ) -> anyhow::Result<i64> {
        let project_id = config.project_id;
        sqlx::query_scalar(
            "SELECT project_id FROM xingtu_project WHERE project_id = $1 AND is_active = true",
        )
        .bind(project_id)
        .fetch_optional(&mut **tx)
        .await?
        .ok_or_else(|| anyhow!("主项目不存在或未启用：project_id={project_id}"))
    }

    /// 将活动配置里的项目星图账号先写入账号表。
    ///
    /// 期次表通过 `xingtu_account_id` 引用它；如果旧配置没有填写，
    /// 就用项目名的小写作为默认账号 ID，便于兼容早期调试 JSON。
    async fn upsert_project_account(
        &self,
        tx: &mut Transaction<'_, Postgres>,
        project_id: i64,
        config: &ActivityConfig,
    ) -> anyhow::Result<String> {
        if let Some(account_id) = config.xingtu_account_id.as_deref() {
            let account_id = account_id.trim();
            let exists: bool = sqlx::query_scalar(
                r#"
                SELECT EXISTS (
                    SELECT 1
                    FROM xingtu_project_account
                    WHERE project_id = $1 AND xingtu_account_id = $2 AND is_active = true
                )
                "#,
            )
            .bind(project_id)
            .bind(account_id)
            .fetch_one(&mut **tx)
            .await?;
            if !exists {
                return Err(anyhow!(
                    "主项目 project_id={project_id} 下不存在启用的星图账号 `{account_id}`"
                ));
            }
            return Ok(account_id.to_owned());
        }

        sqlx::query_scalar(
            r#"
            SELECT xingtu_account_id
            FROM xingtu_project_account
            WHERE project_id = $1 AND is_active = true
            ORDER BY is_default DESC, created_at, xingtu_account_id
            LIMIT 1
            "#,
        )
        .bind(project_id)
        .fetch_optional(&mut **tx)
        .await?
        .ok_or_else(|| anyhow!("主项目 project_id={project_id} 没有启用的星图账号"))
    }

    pub(crate) async fn upsert_contents(
        &self,
        tx: &mut Transaction<'_, Postgres>,
        activity_period_id: i64,
        contents: &[ActivityContentConfig],
    ) -> anyhow::Result<()> {
        for content in contents {
            let spreadsheet_url = trim_optional(content.source.spreadsheet_url.as_deref());
            let manual_table_id = trim_optional(content.tables.manual_table_id.as_deref());
            let task_name = trim_optional(content.xingtu_task.task_name.as_deref());

            let task_id = content.xingtu_task.task_id.trim();
            let existing_task = sqlx::query(
                "SELECT activity_period_id, content_type::text AS content_type FROM xingtu_activity_content_config WHERE xingtu_task_id = $1",
            )
            .bind(task_id)
            .fetch_optional(&mut **tx)
            .await?;
            if let Some(existing_task) = existing_task {
                let existing_period_id: i64 = existing_task.try_get("activity_period_id")?;
                let existing_content_type: String = existing_task.try_get("content_type")?;
                if existing_period_id != activity_period_id
                    || existing_content_type != content.content_type.as_db_value()
                {
                    return Err(anyhow!(
                        "星图任务 `{task_id}` 已属于期次 {existing_period_id} 的 {existing_content_type} 内容，不能重复使用"
                    ));
                }
            }

            sqlx::query(
                r#"
                INSERT INTO xingtu_activity_content_config (
                    activity_period_id,
                    content_type,
                    xingtu_task_id,
                    xingtu_task_name,
                    source_spreadsheet_url,
                    source_spreadsheet_url_update_mode,
                    source_spreadsheet_url_updated_at,
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
                    trace_enabled
                )
                VALUES (
                    $1,
                    $2::xingtu_content_type,
                    $3,
                    $4,
                    $5,
                    $6::xingtu_spreadsheet_url_update_mode,
                    CASE WHEN $5::text IS NULL THEN NULL ELSE now() END,
                    $7,
                    $8,
                    $9,
                    $10,
                    $11,
                    $12,
                    $13,
                    $14,
                    $15,
                    $16,
                    $17
                )
                ON CONFLICT (activity_period_id, content_type)
                DO UPDATE SET
                    xingtu_task_id = EXCLUDED.xingtu_task_id,
                    activity_period_id = EXCLUDED.activity_period_id,
                    content_type = EXCLUDED.content_type,
                    xingtu_task_name = EXCLUDED.xingtu_task_name,
                    source_spreadsheet_url = COALESCE(EXCLUDED.source_spreadsheet_url, xingtu_activity_content_config.source_spreadsheet_url),
                    source_spreadsheet_url_update_mode = EXCLUDED.source_spreadsheet_url_update_mode,
                    source_spreadsheet_url_updated_at = CASE
                        WHEN EXCLUDED.source_spreadsheet_url IS NULL THEN xingtu_activity_content_config.source_spreadsheet_url_updated_at
                        WHEN EXCLUDED.source_spreadsheet_url IS DISTINCT FROM xingtu_activity_content_config.source_spreadsheet_url THEN now()
                        ELSE xingtu_activity_content_config.source_spreadsheet_url_updated_at
                    END,
                    manual_table_id = EXCLUDED.manual_table_id,
                    main_table_id = EXCLUDED.main_table_id,
                    audit_table_id = EXCLUDED.audit_table_id,
                    data_source_field = EXCLUDED.data_source_field,
                    spreadsheet_source_value = EXCLUDED.spreadsheet_source_value,
                    manual_source_value = EXCLUDED.manual_source_value,
                    manual_overrides_spreadsheet = EXCLUDED.manual_overrides_spreadsheet,
                    manual_auto_approve = EXCLUDED.manual_auto_approve,
                    manual_auto_approve_result = EXCLUDED.manual_auto_approve_result,
                    sync_enabled = EXCLUDED.sync_enabled,
                    trace_enabled = EXCLUDED.trace_enabled
                WHERE xingtu_activity_content_config.activity_period_id = EXCLUDED.activity_period_id
                "#,
            )
            .bind(activity_period_id)
            .bind(content.content_type.as_db_value())
            .bind(task_id)
            .bind(task_name)
            .bind(spreadsheet_url)
            .bind(content.source.spreadsheet_url_update_mode.trim())
            .bind(manual_table_id)
            .bind(content.tables.main_table_id.trim())
            .bind(content.tables.audit_table_id.trim())
            .bind(content.sync_rule.data_source_field.trim())
            .bind(content.sync_rule.spreadsheet_source_value.trim())
            .bind(content.sync_rule.manual_source_value.trim())
            .bind(content.sync_rule.manual_overrides_spreadsheet)
            .bind(content.sync_rule.manual_auto_approve)
            .bind(content.sync_rule.manual_auto_approve_result.trim())
            .bind(content.sync_enabled)
            .bind(content.trace_enabled)
            .execute(&mut **tx)
            .await
            .with_context(|| {
                format!(
                    "写入内容配置失败：{} {}",
                    content.content_type.as_db_value(),
                    content.xingtu_task.task_id
                )
            })?;
        }

        let submitted_task_ids = contents
            .iter()
            .map(|content| content.xingtu_task.task_id.trim().to_owned())
            .collect::<Vec<_>>();
        sqlx::query(
            r#"
            UPDATE xingtu_activity_content_config
            SET sync_enabled = false, trace_enabled = false
            WHERE activity_period_id = $1
                AND NOT (xingtu_task_id = ANY($2::text[]))
            "#,
        )
        .bind(activity_period_id)
        .bind(submitted_task_ids)
        .execute(&mut **tx)
        .await
        .context("停用完整配置中已移除的内容项失败")?;

        Ok(())
    }
}

pub fn validate_activity_config(config: &ActivityConfig) -> anyhow::Result<()> {
    if config.project_id <= 0 {
        return Err(anyhow!("project_id 必须是正整数"));
    }
    if config
        .xingtu_account_id
        .as_deref()
        .is_some_and(|account_id| account_id.trim().is_empty())
    {
        return Err(anyhow!("xingtu_account_id 不能是空字符串"));
    }

    if config.period.trim().is_empty() {
        return Err(anyhow!("period 不能为空"));
    }

    if config.bitable_url.trim().is_empty() {
        return Err(anyhow!("bitable_url 不能为空"));
    }
    validate_https_url(&config.bitable_url, "bitable_url")?;

    if config
        .tracking_start_date
        .zip(config.tracking_end_date)
        .is_some_and(|(start, end)| start > end)
    {
        return Err(anyhow!("tracking_start_date 不能晚于 tracking_end_date"));
    }

    if config
        .cpm_table_id
        .as_deref()
        .is_some_and(|value| value.trim().is_empty())
    {
        return Err(anyhow!("cpm_table_id 不能是空字符串"));
    }

    if config.workflows.periodic_sync_interval_hours <= 0 {
        return Err(anyhow!("periodic_sync_interval_hours 必须大于 0"));
    }

    validate_activity_contents(&config.contents)?;

    Ok(())
}

pub fn validate_activity_contents(contents: &[ActivityContentConfig]) -> anyhow::Result<()> {
    if contents.is_empty() {
        return Err(anyhow!("contents 不能为空"));
    }
    let mut seen_content_type = HashSet::new();
    let mut task_ids = HashSet::new();
    for content in contents {
        if !seen_content_type.insert(content.content_type.as_db_value()) {
            return Err(anyhow!(
                "同一期活动不能重复配置 content_type={}",
                content.content_type.as_db_value()
            ));
        }

        if content.xingtu_task.task_id.trim().is_empty() {
            return Err(anyhow!("xingtu_task.task_id 不能为空"));
        }
        if !task_ids.insert(content.xingtu_task.task_id.trim()) {
            return Err(anyhow!(
                "xingtu_task.task_id 重复：{}",
                content.xingtu_task.task_id
            ));
        }
        if !matches!(
            content.source.spreadsheet_url_update_mode.trim(),
            "xingtu_export" | "manual"
        ) {
            return Err(anyhow!(
                "spreadsheet_url_update_mode 只支持 xingtu_export 或 manual"
            ));
        }
        if let Some(url) = content
            .source
            .spreadsheet_url
            .as_deref()
            .filter(|url| !url.trim().is_empty())
        {
            validate_https_url(url, "source.spreadsheet_url")?;
        }

        if content.tables.main_table_id.trim().is_empty() {
            return Err(anyhow!("main_table_id 不能为空"));
        }

        if content.tables.audit_table_id.trim().is_empty() {
            return Err(anyhow!("audit_table_id 不能为空"));
        }
    }

    Ok(())
}

fn validate_https_url(value: &str, field: &str) -> anyhow::Result<()> {
    let url = Url::parse(value.trim()).with_context(|| format!("{field} 不是合法 URL"))?;
    if url.scheme() != "https" || url.host_str().is_none() {
        return Err(anyhow!("{field} 必须是带域名的 HTTPS URL"));
    }
    Ok(())
}

fn parse_content_type(value: &str) -> anyhow::Result<XingtuContentType> {
    match value {
        "live" => Ok(XingtuContentType::Live),
        "video" => Ok(XingtuContentType::Video),
        other => Err(anyhow!("未知 content_type：{other}")),
    }
}

fn build_activity_table_sync_config(
    content: SyncableContentConfig,
    data_import_repo: XingtuDataImportRepository,
    source_mode: ActivityTableSyncSourceMode,
) -> ActivityTableSyncConfig {
    let sync_rule = ActivityTableSyncRuleConfig {
        data_source_field: content.data_source_field,
        spreadsheet_source_value: content.spreadsheet_source_value,
        manual_source_value: content.manual_source_value.clone(),
        manual_overrides_spreadsheet: content.manual_overrides_spreadsheet,
        manual_auto_approve: content.manual_auto_approve,
        manual_auto_approve_result: content.manual_auto_approve_result.clone(),
        audit_result_field: content.audit_result_field,
    };

    let stat_date = content
        .latest_stat_date
        .unwrap_or_else(|| now_utc().date_naive());
    let mut persist_options = PersistMergedRecordsOptions::new(
        content.content_config_id,
        content.content_type.as_db_value(),
        content.latest_feishu_source_id,
        stat_date,
    );
    persist_options.is_daily_final = content.latest_is_daily_final;
    persist_options.manual_source_value = content.manual_source_value;
    persist_options.manual_auto_approve = content.manual_auto_approve;
    persist_options.manual_auto_approve_result = content.manual_auto_approve_result;
    persist_options.trace_enabled = content.trace_enabled;

    ActivityTableSyncConfig {
        bitable_url: content.bitable_url,
        spreadsheet_url: content.source_spreadsheet_url,
        source_mode,
        manual_table_id: content.manual_table_id,
        main_table_id: content.main_table_id,
        audit_table_id: Some(content.audit_table_id),
        sync_rule,
        db_sync: Some(ActivityTableDbSyncConfig {
            repository: data_import_repo,
            options: persist_options,
        }),
    }
}

fn trim_optional(value: Option<&str>) -> Option<String> {
    value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

fn default_true() -> bool {
    true
}

fn default_periodic_sync_interval_hours() -> i32 {
    2
}

fn default_spreadsheet_url_update_mode() -> String {
    "xingtu_export".to_string()
}

fn default_data_source_field() -> String {
    "数据来源".to_string()
}

fn default_spreadsheet_source_value() -> String {
    "星图数据".to_string()
}

fn default_manual_source_value() -> String {
    "手动登记".to_string()
}

fn default_manual_auto_approve_result() -> String {
    "审核通过".to_string()
}

/// 判断一次星图拉取是否落在活动追踪窗口内。
///
/// `tracking_end_date` 当天结束后，额外保留北京时间 T+1 的 03:00 小时，
/// 供 03:00 定时任务完成最后一次更新；09:00 及后续时段不再拉取。
fn tracking_window_includes(
    run_at: DateTime<Utc>,
    tracking_start_date: Option<NaiveDate>,
    tracking_end_date: Option<NaiveDate>,
) -> bool {
    let local = run_at.with_timezone(&Shanghai);
    let local_date = local.date_naive();

    if tracking_start_date.is_some_and(|start_date| local_date < start_date) {
        return false;
    }

    let Some(end_date) = tracking_end_date else {
        return true;
    };
    if local_date <= end_date {
        return true;
    }

    end_date.succ_opt() == Some(local_date) && local.hour() == 3
}

/// 当前时间，测试时可通过单独函数替换调用点。
pub fn now_utc() -> chrono::DateTime<Utc> {
    Utc::now()
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn beijing_time(year: i32, month: u32, day: u32, hour: u32, minute: u32) -> DateTime<Utc> {
        Shanghai
            .with_ymd_and_hms(year, month, day, hour, minute, 0)
            .single()
            .expect("valid Beijing time")
            .with_timezone(&Utc)
    }

    #[test]
    fn tracking_end_date_includes_only_next_day_three_oclock_window() {
        let end_date = NaiveDate::from_ymd_opt(2026, 8, 3).expect("valid date");

        assert!(tracking_window_includes(
            beijing_time(2026, 8, 3, 23, 59),
            None,
            Some(end_date),
        ));
        assert!(tracking_window_includes(
            beijing_time(2026, 8, 4, 3, 0),
            None,
            Some(end_date),
        ));
        assert!(tracking_window_includes(
            beijing_time(2026, 8, 4, 3, 59),
            None,
            Some(end_date),
        ));
        assert!(!tracking_window_includes(
            beijing_time(2026, 8, 4, 9, 0),
            None,
            Some(end_date),
        ));
        assert!(!tracking_window_includes(
            beijing_time(2026, 8, 5, 3, 0),
            None,
            Some(end_date),
        ));
    }

    #[test]
    fn tracking_start_date_still_excludes_earlier_runs() {
        let start_date = NaiveDate::from_ymd_opt(2026, 8, 3).expect("valid date");

        assert!(!tracking_window_includes(
            beijing_time(2026, 8, 2, 21, 0),
            Some(start_date),
            None,
        ));
        assert!(tracking_window_includes(
            beijing_time(2026, 8, 3, 3, 0),
            Some(start_date),
            None,
        ));
    }

    #[test]
    fn example_config_is_a_valid_fixture_and_keeps_xingtu_priority() {
        let configs: Vec<ActivityConfig> =
            serde_json::from_str(include_str!("../../a-config-example.json")).unwrap();
        assert!(!configs.is_empty());
        for config in configs {
            validate_activity_config(&config).unwrap();
            assert!(
                config
                    .contents
                    .iter()
                    .all(|content| !content.sync_rule.manual_overrides_spreadsheet)
            );
        }
    }

    #[test]
    fn activity_config_rejects_terminated_project_level_fields() {
        let mut value: serde_json::Value =
            serde_json::from_str(include_str!("../../a-config-example.json")).unwrap();
        value[0]["project"] = serde_json::json!("ROK");
        assert!(serde_json::from_value::<Vec<ActivityConfig>>(value).is_err());
    }
}
