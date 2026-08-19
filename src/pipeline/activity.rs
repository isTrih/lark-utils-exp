use crate::lark::im::MessageReceiver;
use crate::xingtu::activity_config::XingtuContentType;
use crate::xingtu::data_import::{PersistMergedRecordsOptions, XingtuDataImportRepository};

/// 默认审核结果字段名。
///
/// 审核通知流程会统计这个字段为空的记录数，作为待审核数量。
pub const DEFAULT_AUDIT_RESULT_FIELD: &str = "审核结果";
pub const DEFAULT_DATA_SOURCE_FIELD: &str = "数据来源";
pub const DEFAULT_SPREADSHEET_SOURCE_VALUE: &str = "星图数据";
pub const DEFAULT_MANUAL_SOURCE_VALUE: &str = "手动登记";
pub const DEFAULT_MANUAL_AUTO_APPROVE_RESULT: &str = "审核通过";

/// 活动表同步时读取的数据来源范围。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum ActivityTableSyncSourceMode {
    /// 同时读取当前星图 Sheet 和手动登记表。
    #[default]
    SpreadsheetAndManual,
    /// 只读取手动登记表，不访问当前星图 Sheet。
    ManualOnly,
}

impl ActivityTableSyncSourceMode {
    pub fn reads_spreadsheet(self) -> bool {
        matches!(self, Self::SpreadsheetAndManual)
    }
}

/// 单个内容类型在某一期活动中的同步配置。
///
/// 这些字段会随着每期活动变化；业务固定项（例如唯一键字段、字段类型规则）
/// 不放在这里，而是固定在视频/直播各自的 pipeline 模块中。
#[derive(Debug, Clone)]
pub struct ActivityTableSyncConfig {
    pub bitable_url: String,
    pub spreadsheet_url: String,
    pub source_mode: ActivityTableSyncSourceMode,
    pub manual_table_id: Option<String>,
    pub main_table_id: String,
    pub audit_table_id: Option<String>,
    pub sync_rule: ActivityTableSyncRuleConfig,
    pub db_sync: Option<ActivityTableDbSyncConfig>,
}

/// 单个内容表同步规则。
///
/// 这些默认值来自当前活动表设计；后续每期活动如果字段名或文案变化，
/// 调度器可以从数据库配置传入不同值。
#[derive(Debug, Clone)]
pub struct ActivityTableSyncRuleConfig {
    pub data_source_field: String,
    pub spreadsheet_source_value: String,
    pub manual_source_value: String,
    pub manual_overrides_spreadsheet: bool,
    pub manual_auto_approve: bool,
    pub manual_auto_approve_result: String,
    pub audit_result_field: String,
}

impl Default for ActivityTableSyncRuleConfig {
    fn default() -> Self {
        Self {
            data_source_field: DEFAULT_DATA_SOURCE_FIELD.to_string(),
            spreadsheet_source_value: DEFAULT_SPREADSHEET_SOURCE_VALUE.to_string(),
            manual_source_value: DEFAULT_MANUAL_SOURCE_VALUE.to_string(),
            manual_overrides_spreadsheet: false,
            manual_auto_approve: true,
            manual_auto_approve_result: DEFAULT_MANUAL_AUTO_APPROVE_RESULT.to_string(),
            audit_result_field: DEFAULT_AUDIT_RESULT_FIELD.to_string(),
        }
    }
}

/// 同步 pipeline 的可选数据库入库配置。
///
/// 手动 006 调试可以不传；后续调度器从数据库任务触发时传入这份配置，
/// pipeline 就会在“手动合并完成后、写入飞书主表前”先把合并结果落库。
#[derive(Debug, Clone)]
pub struct ActivityTableDbSyncConfig {
    pub repository: XingtuDataImportRepository,
    pub options: PersistMergedRecordsOptions,
}

/// 单个审核表的通知统计配置。
///
/// 这里不区分直播或视频，只描述“去哪个多维表格的哪个 audit 表统计待审核数”。
#[derive(Debug, Clone)]
pub struct ActivityAuditTableConfig {
    pub bitable_url: String,
    pub audit_table_id: String,
}

/// 从审核表回写数据库所需的最小配置。
#[derive(Debug, Clone)]
pub struct ActivityAuditResultSyncConfig {
    pub period: String,
    pub content_config_id: i64,
    pub content_type: XingtuContentType,
    pub bitable_url: String,
    pub audit_table_id: String,
    pub audit_result_field: String,
}

/// 单期活动的审核通知配置。
///
/// `audit_table_url` 是卡片里展示给审核同学点击的链接，通常传入飞书 markdown 链接。
#[derive(Debug, Clone)]
pub struct ActivityAuditNoticeConfig {
    pub period: String,
    pub audit_table_url: String,
    pub live: Option<ActivityAuditTableConfig>,
    pub video: Option<ActivityAuditTableConfig>,
}

/// 审核通知 workflow 配置。
///
/// 后续调度器可以读取这份配置：
/// - 接收者来自任务配置
/// - 每期活动要统计的直播/视频 audit 表来自任务配置
/// - 审核结果字段名默认是 `审核结果`，字段变化时也可以覆盖
#[derive(Debug, Clone)]
pub struct AuditNoticeWorkflowConfig {
    pub receiver: MessageReceiver,
    pub card_template_id: String,
    pub auditor_ids: String,
    pub project_name: String,
    pub audit_result_field: String,
    pub activities: Vec<ActivityAuditNoticeConfig>,
}

impl AuditNoticeWorkflowConfig {
    /// 使用默认审核结果字段创建审核通知配置。
    pub fn new(
        receiver: MessageReceiver,
        card_template_id: impl Into<String>,
        auditor_ids: impl Into<String>,
        project_name: impl Into<String>,
        activities: Vec<ActivityAuditNoticeConfig>,
    ) -> Self {
        Self {
            receiver,
            card_template_id: card_template_id.into(),
            auditor_ids: auditor_ids.into(),
            project_name: project_name.into(),
            audit_result_field: DEFAULT_AUDIT_RESULT_FIELD.to_string(),
            activities,
        }
    }
}

/// 某一期活动的完整同步配置。
///
/// 调度器后续可以从数据库或任务配置中读取这份结构，再触发固定工作流。
/// 当前 006 测试入口会手动构造它，模拟调度器触发。
#[derive(Debug, Clone, Default)]
pub struct ActivitySyncConfig {
    pub period: String,
    pub live: Option<ActivityTableSyncConfig>,
    pub video: Option<ActivityTableSyncConfig>,
}

/// 每日早上审核 workflow 配置。
///
/// 同步和通知分开配置：
/// - `sync_activities`：先按顺序同步多期活动
/// - `audit_notice`：全部同步完成后，再统一统计待审核数量并通知
#[derive(Debug, Clone, Default)]
pub struct DailyMorningReviewWorkflowConfig {
    pub sync_activities: Vec<ActivitySyncConfig>,
    pub audit_notice: Option<AuditNoticeWorkflowConfig>,
}
