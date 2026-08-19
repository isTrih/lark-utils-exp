use crate::client::LarkClient;
use crate::lark::sheets::get_spreadsheet_token;
use crate::pipeline::activity::{ActivityTableSyncRuleConfig, ActivityTableSyncSourceMode};
use crate::pipeline::bitable::{
    batch_create_records, batch_update_records, read_existing_records_with_retry,
    read_optional_records_with_retry,
};
use crate::pipeline::runner::{Pipeline, PipelineStep, StepFuture};
use crate::pipeline::sheet::read_first_sheet_rows;
use crate::pipeline::sync::{
    ExistingRecordIndexItem, FieldValueRules, SourceRow, UpsertPlan, build_upsert_plan,
    index_existing_records, normalize_field_name, parse_bitable_records_as_source_rows,
    parse_sheet_rows, value_to_plain_string,
};
use crate::xingtu::data_import::MergedRecordForDb;
use anyhow::{Context, anyhow};
use open_lark::docs::base::bitable::{CreateRecordItem, Record as BitableRecord, UpdateRecordItem};
use serde_json::{Map, Value};
use std::collections::{HashMap, HashSet};

/// 单条 Sheet -> 多维表格同步 pipeline 的内部配置。
///
/// 这个结构只在 pipeline 内部复用，业务侧仍然通过 video/live/workflow 的入口触发，
/// 避免把外部调用方式做成没有业务语义的“万能同步器”。
#[derive(Debug, Clone)]
pub(crate) struct TableSyncConfig {
    /// pipeline 名称，用于日志和错误定位。
    pub pipeline_name: &'static str,
    /// 本期活动的多维表格 URL，每期活动会变化。
    pub bitable_url: String,
    /// 本期活动的源数据 Sheet URL，每期活动会变化。
    pub spreadsheet_url: String,
    /// 本轮同步读取完整来源还是只读取手动登记。
    pub source_mode: ActivityTableSyncSourceMode,
    /// 本期活动的手动登记表 ID，每期活动会变化。
    pub manual_table_id: Option<String>,
    /// 目标主表 ID，每期活动会变化。
    pub main_table_id: String,
    /// 目标 audit 表 ID，不需要 audit 时可以为空。
    pub audit_table_id: Option<String>,
    /// 唯一键字段名由具体业务 pipeline 固定，不放到每期活动配置里。
    pub unique_key_field: &'static str,
    /// audit 日志里展示的唯一键名称。
    pub audit_key_label: &'static str,
    /// 本期活动的同步规则，可由数据库配置传入。
    pub sync_rule: ActivityTableSyncRuleConfig,
    /// 可选数据库入库配置。没有配置时只同步飞书，不落业务库。
    pub db_sync: Option<crate::pipeline::activity::ActivityTableDbSyncConfig>,
}

/// 单条 Sheet -> 多维表格同步 pipeline 的共享上下文。
///
/// 每个 step 只读写自己负责的字段，这样 step 顺序和数据依赖会比较清楚。
struct TableSyncContext {
    lark: LarkClient,
    config: TableSyncConfig,
    rules: FieldValueRules,
    app_token: String,
    spreadsheet_token: Option<String>,
    sheet_rows: Vec<Vec<Value>>,
    manual_records: Vec<BitableRecord>,
    source_rows: Vec<SourceRow>,
    audit_source_rows: Vec<AuditSourceRow>,
    existing_main_loaded: bool,
    existing_records: Vec<BitableRecord>,
    existing_index: HashMap<String, ExistingRecordIndexItem>,
    upsert_plan: UpsertPlan,
}

/// audit 表同步所需的最小源数据。
#[derive(Debug, Clone)]
struct AuditSourceRow {
    unique_key: String,
    data_source: Option<String>,
}

impl TableSyncContext {
    /// 构建同步上下文，并提前解析飞书 token。
    fn new(
        lark: LarkClient,
        config: TableSyncConfig,
        rules: FieldValueRules,
    ) -> anyhow::Result<Self> {
        let app_token =
            get_spreadsheet_token(&config.bitable_url).context("解析多维表格 app_token 失败")?;
        let spreadsheet_token = if config.source_mode.reads_spreadsheet() {
            Some(
                get_spreadsheet_token(&config.spreadsheet_url)
                    .context("解析电子表格 spreadsheet_token 失败")?,
            )
        } else {
            None
        };

        Ok(Self {
            lark,
            config,
            rules,
            app_token,
            spreadsheet_token,
            sheet_rows: Vec::new(),
            manual_records: Vec::new(),
            source_rows: Vec::new(),
            audit_source_rows: Vec::new(),
            existing_main_loaded: false,
            existing_records: Vec::new(),
            existing_index: HashMap::new(),
            upsert_plan: UpsertPlan::default(),
        })
    }
}

/// 运行一条具体业务 pipeline 的通用同步段。
///
/// video/live 会把自己的固定字段规则和唯一键传进来；每期活动变化的 URL/表 ID
/// 则来自调度器或当前手动触发代码提供的配置。
pub(crate) async fn run_table_sync_pipeline(
    lark: LarkClient,
    config: TableSyncConfig,
    rules: FieldValueRules,
) -> anyhow::Result<()> {
    let mut ctx = TableSyncContext::new(lark, config, rules)?;

    tracing::debug!("app_token: {}", ctx.app_token);
    if let Some(spreadsheet_token) = ctx.spreadsheet_token.as_deref() {
        tracing::debug!("spreadsheet_token: {}", spreadsheet_token);
    }

    build_table_sync_pipeline(ctx.config.pipeline_name)
        .run(&mut ctx)
        .await?;

    tracing::info!("{} 同步完成", ctx.config.pipeline_name);
    Ok(())
}

/// 组装内部同步 pipeline。
fn build_table_sync_pipeline(pipeline_name: &'static str) -> Pipeline<TableSyncContext> {
    Pipeline::new(pipeline_name)
        .add_step(ReadSourceSheetStep)
        .add_step(ReadManualRecordsStep)
        .add_step(BuildSourceRowsStep)
        .add_step(LoadExistingMainRecordsForManualOnlyStep)
        .add_step(PreserveExistingSpreadsheetRowsStep)
        .add_step(PersistMergedRecordsStep)
        .add_step(LoadExistingMainRecordsStep)
        .add_step(BuildMainUpsertPlanStep)
        .add_step(ApplyMainUpsertPlanStep)
        .add_step(SyncAuditTableStep)
}

/// step：读取源 Sheet 原始数据。
struct ReadSourceSheetStep;

impl PipelineStep<TableSyncContext> for ReadSourceSheetStep {
    fn name(&self) -> &'static str {
        "read_source_sheet"
    }

    fn run<'a>(&'a self, ctx: &'a mut TableSyncContext) -> StepFuture<'a> {
        Box::pin(async move {
            let Some(spreadsheet_token) = ctx.spreadsheet_token.as_deref() else {
                tracing::info!("本轮仅同步手动登记，跳过星图 Sheet 读取");
                return Ok(());
            };

            ctx.sheet_rows = read_first_sheet_rows(&ctx.lark, spreadsheet_token)
                .await
                .context("读取 Sheet 数据失败")?;

            tracing::info!("Sheet 原始行数：{}", ctx.sheet_rows.len());
            Ok(())
        })
    }
}

/// step：读取手动登记表数据。
struct ReadManualRecordsStep;

impl PipelineStep<TableSyncContext> for ReadManualRecordsStep {
    fn name(&self) -> &'static str {
        "read_manual_records"
    }

    fn run<'a>(&'a self, ctx: &'a mut TableSyncContext) -> StepFuture<'a> {
        Box::pin(async move {
            let Some(manual_table_id) = ctx.config.manual_table_id.as_deref() else {
                tracing::info!("未配置手动登记表，跳过手动登记数据读取");
                return Ok(());
            };

            ctx.manual_records =
                read_optional_records_with_retry(&ctx.lark, &ctx.app_token, manual_table_id)
                    .await
                    .context("读取手动登记表记录失败")?;

            tracing::info!("手动登记表原始记录数：{}", ctx.manual_records.len());
            Ok(())
        })
    }
}

/// step：解析 Sheet 行，并提前收集 audit 表需要的唯一键。
struct BuildSourceRowsStep;

impl PipelineStep<TableSyncContext> for BuildSourceRowsStep {
    fn name(&self) -> &'static str {
        "build_source_rows"
    }

    fn run<'a>(&'a self, ctx: &'a mut TableSyncContext) -> StepFuture<'a> {
        Box::pin(async move {
            let sheet_rows = std::mem::take(&mut ctx.sheet_rows);
            let mut spreadsheet_rows = parse_spreadsheet_source_rows(
                ctx.config.source_mode,
                sheet_rows,
                ctx.config.unique_key_field,
                &ctx.rules,
            )?;
            attach_data_source(
                &mut spreadsheet_rows,
                &ctx.config.sync_rule.data_source_field,
                &ctx.config.sync_rule.spreadsheet_source_value,
            );

            let manual_records = std::mem::take(&mut ctx.manual_records);
            let mut manual_rows = parse_bitable_records_as_source_rows(
                &manual_records,
                ctx.config.unique_key_field,
                &ctx.rules,
            )
            .context("解析手动登记表记录失败")?;
            attach_data_source(
                &mut manual_rows,
                &ctx.config.sync_rule.data_source_field,
                &ctx.config.sync_rule.manual_source_value,
            );

            tracing::info!("星图有效源数据行数：{}", spreadsheet_rows.len());
            tracing::info!("手动登记有效源数据行数：{}", manual_rows.len());

            ctx.source_rows = merge_source_rows(
                spreadsheet_rows,
                manual_rows,
                ctx.config.sync_rule.manual_overrides_spreadsheet,
            );
            ctx.audit_source_rows = collect_audit_source_rows(
                &ctx.source_rows,
                &ctx.config.sync_rule.data_source_field,
            );

            tracing::info!("合并后有效源数据行数：{}", ctx.source_rows.len());
            Ok(())
        })
    }
}

fn parse_spreadsheet_source_rows(
    source_mode: ActivityTableSyncSourceMode,
    sheet_rows: Vec<Vec<Value>>,
    unique_key_field: &str,
    rules: &FieldValueRules,
) -> anyhow::Result<Vec<SourceRow>> {
    if !source_mode.reads_spreadsheet() {
        return Ok(Vec::new());
    }

    parse_sheet_rows(sheet_rows, unique_key_field, rules).context("解析 Sheet 行数据失败")
}

/// 给源数据行补充主表中的 `数据来源` 字段。
fn attach_data_source(source_rows: &mut [SourceRow], data_source_field: &str, data_source: &str) {
    for row in source_rows {
        row.fields.insert(
            data_source_field.to_string(),
            Value::String(data_source.to_string()),
        );
    }
}

/// 合并星图数据和手动登记数据。
///
/// 默认先放入星图数据，再放入手动登记数据。遇到同一个唯一键时：
/// - `manual_overrides_spreadsheet = true`：手动登记覆盖星图数据
/// - `manual_overrides_spreadsheet = false`：保留先进入的星图数据
fn merge_source_rows(
    spreadsheet_rows: Vec<SourceRow>,
    manual_rows: Vec<SourceRow>,
    manual_overrides_spreadsheet: bool,
) -> Vec<SourceRow> {
    let mut merged_rows = Vec::new();
    let mut key_to_index = HashMap::new();

    for row in spreadsheet_rows {
        key_to_index.insert(row.unique_key.clone(), merged_rows.len());
        merged_rows.push(row);
    }

    for row in manual_rows {
        if let Some(existing_index) = key_to_index.get(&row.unique_key).copied() {
            if manual_overrides_spreadsheet {
                merged_rows[existing_index] = row;
            }
            continue;
        }

        key_to_index.insert(row.unique_key.clone(), merged_rows.len());
        merged_rows.push(row);
    }

    merged_rows
}

/// 收集 audit 表同步需要的唯一键和数据来源。
///
/// audit 表里手动登记数据需要默认写入“审核通过”，所以不能只保留唯一键。
fn collect_audit_source_rows(
    source_rows: &[SourceRow],
    data_source_field: &str,
) -> Vec<AuditSourceRow> {
    let mut rows = Vec::new();
    let mut seen = HashSet::new();

    for row in source_rows {
        if !seen.insert(row.unique_key.clone()) {
            continue;
        }

        rows.push(AuditSourceRow {
            unique_key: row.unique_key.clone(),
            data_source: row
                .fields
                .get(data_source_field)
                .and_then(value_to_plain_string),
        });
    }

    rows
}

/// step：把合并后的源数据写入业务数据库。
///
/// 这个 step 是可选的：006 手动调试不传 db_sync 时会跳过；
/// 调度器从数据库任务触发时传入 db_sync，就会先入库再同步飞书主表。
struct PersistMergedRecordsStep;

impl PipelineStep<TableSyncContext> for PersistMergedRecordsStep {
    fn name(&self) -> &'static str {
        "persist_merged_records"
    }

    fn run<'a>(&'a self, ctx: &'a mut TableSyncContext) -> StepFuture<'a> {
        Box::pin(async move {
            let Some(db_sync) = ctx.config.db_sync.as_ref() else {
                tracing::info!("未配置数据库入库，跳过业务表持久化");
                return Ok(());
            };

            let rows = ctx
                .source_rows
                .iter()
                .cloned()
                .map(|row| {
                    MergedRecordForDb::from_source_row(
                        row,
                        &ctx.config.sync_rule.data_source_field,
                        &ctx.config.sync_rule.spreadsheet_source_value,
                    )
                })
                .collect::<Vec<_>>();
            let mut options = db_sync.options.clone();
            options.manual_source_value = ctx.config.sync_rule.manual_source_value.clone();
            options.manual_auto_approve = ctx.config.sync_rule.manual_auto_approve;
            options.manual_auto_approve_result =
                ctx.config.sync_rule.manual_auto_approve_result.clone();

            let persisted = db_sync
                .repository
                .persist_merged_records(options, rows)
                .await
                .context("合并后数据写入业务数据库失败")?;

            tracing::info!(
                persisted_rows = persisted.persisted_rows,
                quarantined_rows = persisted.quarantined_rows,
                "合并后数据已写入业务数据库"
            );
            Ok(())
        })
    }
}

/// step：读取主表已有记录并建立唯一键索引。
struct LoadExistingMainRecordsStep;

impl PipelineStep<TableSyncContext> for LoadExistingMainRecordsStep {
    fn name(&self) -> &'static str {
        "load_existing_main_records"
    }

    fn run<'a>(&'a self, ctx: &'a mut TableSyncContext) -> StepFuture<'a> {
        Box::pin(async move { load_existing_main_records(ctx).await })
    }
}

/// 手动专项同步需要在入库前识别主表中的星图记录。
struct LoadExistingMainRecordsForManualOnlyStep;

impl PipelineStep<TableSyncContext> for LoadExistingMainRecordsForManualOnlyStep {
    fn name(&self) -> &'static str {
        "load_existing_main_records_for_manual_only"
    }

    fn run<'a>(&'a self, ctx: &'a mut TableSyncContext) -> StepFuture<'a> {
        Box::pin(async move {
            if ctx.config.source_mode == ActivityTableSyncSourceMode::ManualOnly {
                load_existing_main_records(ctx).await?;
            }
            Ok(())
        })
    }
}

async fn load_existing_main_records(ctx: &mut TableSyncContext) -> anyhow::Result<()> {
    if ctx.existing_main_loaded {
        return Ok(());
    }

    ctx.existing_records =
        read_existing_records_with_retry(&ctx.lark, &ctx.app_token, &ctx.config.main_table_id)
            .await
            .context("读取多维表格已有记录失败")?;
    ctx.existing_main_loaded = true;

    tracing::info!("多维表格已有记录数：{}", ctx.existing_records.len());

    ctx.existing_index = index_existing_records(&ctx.existing_records, ctx.config.unique_key_field)
        .context("建立业务主表唯一键索引失败")?;

    tracing::info!("可匹配已有记录数：{}", ctx.existing_index.len());

    if !ctx.existing_records.is_empty() && ctx.existing_index.is_empty() {
        return Err(anyhow!(
            "已有记录数量为 {}，但唯一键 `{}` 匹配数量为 0。\
             这通常表示字段名不一致或字段值结构无法解析。已中止，避免误创建。",
            ctx.existing_records.len(),
            ctx.config.unique_key_field
        ));
    }

    Ok(())
}

/// 仅手动同步时，主表中已经属于星图的数据继续以星图为准。
struct PreserveExistingSpreadsheetRowsStep;

impl PipelineStep<TableSyncContext> for PreserveExistingSpreadsheetRowsStep {
    fn name(&self) -> &'static str {
        "preserve_existing_spreadsheet_rows"
    }

    fn run<'a>(&'a self, ctx: &'a mut TableSyncContext) -> StepFuture<'a> {
        Box::pin(async move {
            if ctx.config.source_mode != ActivityTableSyncSourceMode::ManualOnly {
                return Ok(());
            }

            let spreadsheet_owned_keys = spreadsheet_owned_keys(
                &ctx.existing_index,
                &ctx.config.sync_rule.data_source_field,
                &ctx.config.sync_rule.spreadsheet_source_value,
            );
            if spreadsheet_owned_keys.is_empty() {
                return Ok(());
            }

            let before = ctx.source_rows.len();
            ctx.source_rows
                .retain(|row| !spreadsheet_owned_keys.contains(&row.unique_key));
            ctx.audit_source_rows
                .retain(|row| !spreadsheet_owned_keys.contains(&row.unique_key));

            tracing::info!(
                "手动登记同步跳过主表中已有的星图记录：{} 条",
                before - ctx.source_rows.len()
            );
            Ok(())
        })
    }
}

fn spreadsheet_owned_keys(
    existing_index: &HashMap<String, ExistingRecordIndexItem>,
    data_source_field: &str,
    spreadsheet_source_value: &str,
) -> HashSet<String> {
    existing_index
        .iter()
        .filter_map(|(unique_key, record)| {
            let data_source = record
                .fields
                .iter()
                .find(|(field_name, _)| {
                    normalize_field_name(field_name) == normalize_field_name(data_source_field)
                })
                .and_then(|(_, value)| value_to_plain_string(value));

            data_source
                .is_some_and(|value| value.trim() == spreadsheet_source_value.trim())
                .then(|| unique_key.clone())
        })
        .collect()
}

/// step：根据源数据和主表索引生成 upsert 计划。
struct BuildMainUpsertPlanStep;

impl PipelineStep<TableSyncContext> for BuildMainUpsertPlanStep {
    fn name(&self) -> &'static str {
        "build_main_upsert_plan"
    }

    fn run<'a>(&'a self, ctx: &'a mut TableSyncContext) -> StepFuture<'a> {
        Box::pin(async move {
            let source_rows = std::mem::take(&mut ctx.source_rows);
            let existing_index = std::mem::take(&mut ctx.existing_index);
            ctx.upsert_plan = build_upsert_plan(source_rows, existing_index, &ctx.rules);

            tracing::info!(
                "同步计划：create={} update={}",
                ctx.upsert_plan.need_create_records.len(),
                ctx.upsert_plan.need_update_records.len()
            );

            Ok(())
        })
    }
}

/// step：执行主表批量更新和批量创建。
struct ApplyMainUpsertPlanStep;

impl PipelineStep<TableSyncContext> for ApplyMainUpsertPlanStep {
    fn name(&self) -> &'static str {
        "apply_main_upsert_plan"
    }

    fn run<'a>(&'a self, ctx: &'a mut TableSyncContext) -> StepFuture<'a> {
        Box::pin(async move {
            let update_records = std::mem::take(&mut ctx.upsert_plan.need_update_records);
            batch_update_records(
                &ctx.lark,
                &ctx.app_token,
                &ctx.config.main_table_id,
                update_records,
            )
            .await
            .context("执行批量更新失败")?;

            let create_records = std::mem::take(&mut ctx.upsert_plan.need_create_records);
            batch_create_records(
                &ctx.lark,
                &ctx.app_token,
                &ctx.config.main_table_id,
                create_records,
            )
            .await
            .context("执行批量创建失败")?;

            Ok(())
        })
    }
}

/// step：同步 audit 表。
///
/// audit 表目前只维护唯一键字段，主表完成后再补齐 audit 表缺失记录。
struct SyncAuditTableStep;

impl PipelineStep<TableSyncContext> for SyncAuditTableStep {
    fn name(&self) -> &'static str {
        "sync_audit_table"
    }

    fn run<'a>(&'a self, ctx: &'a mut TableSyncContext) -> StepFuture<'a> {
        Box::pin(async move {
            let Some(audit_table_id) = ctx.config.audit_table_id.as_deref() else {
                tracing::info!("未配置 audit 表，跳过 audit 同步");
                return Ok(());
            };

            if ctx.audit_source_rows.is_empty() {
                tracing::info!("audit 表没有需要同步的{}", ctx.config.audit_key_label);
                return Ok(());
            }

            tracing::info!(
                "开始同步 audit 表，源{}数量：{}",
                ctx.config.audit_key_label,
                ctx.audit_source_rows.len()
            );

            let existing_audit_records =
                read_existing_records_with_retry(&ctx.lark, &ctx.app_token, audit_table_id)
                    .await
                    .context("读取 audit 表已有记录失败")?;

            tracing::info!("audit 表已有记录数：{}", existing_audit_records.len());

            let existing_audit_index =
                index_existing_records(&existing_audit_records, ctx.config.unique_key_field)
                    .context("建立 audit 表唯一键索引失败")?;

            tracing::info!("audit 表可匹配记录数：{}", existing_audit_index.len());

            if !existing_audit_records.is_empty() && existing_audit_index.is_empty() {
                return Err(anyhow!(
                    "audit 表已有记录数量为 {}，但唯一键 `{}` 匹配数量为 0。\
                     这通常表示 audit 表字段名不一致或字段值结构无法解析。已中止，避免误创建。",
                    existing_audit_records.len(),
                    ctx.config.unique_key_field
                ));
            }

            let audit_source_rows = std::mem::take(&mut ctx.audit_source_rows);
            let (need_create_records, need_update_records) = build_audit_sync_records(
                audit_source_rows,
                &existing_audit_index,
                ctx.config.unique_key_field,
                &ctx.rules,
                &ctx.config.sync_rule,
            );

            tracing::info!(
                "audit 表同步计划：create={} update={}",
                need_create_records.len(),
                need_update_records.len()
            );

            batch_update_records(
                &ctx.lark,
                &ctx.app_token,
                audit_table_id,
                need_update_records,
            )
            .await
            .context("audit 表批量更新失败")?;

            batch_create_records(
                &ctx.lark,
                &ctx.app_token,
                audit_table_id,
                need_create_records,
            )
            .await
            .context("audit 表批量创建失败")?;

            tracing::info!("audit 表同步完成");
            Ok(())
        })
    }
}

/// 构建 audit 表同步计划。
///
/// 手动登记来源的数据直接写入 `审核结果 = 审核通过`：
/// - audit 表不存在该唯一键：创建时带上审核结果
/// - audit 表已存在该唯一键：如果审核结果还不是审核通过，则补一次更新
fn build_audit_sync_records(
    audit_source_rows: Vec<AuditSourceRow>,
    existing_index: &HashMap<String, ExistingRecordIndexItem>,
    unique_key_field: &str,
    rules: &FieldValueRules,
    sync_rule: &ActivityTableSyncRuleConfig,
) -> (Vec<CreateRecordItem>, Vec<UpdateRecordItem>) {
    let mut need_create_records = Vec::new();
    let mut need_update_records = Vec::new();

    for row in audit_source_rows {
        let is_manual =
            is_manual_data_source(row.data_source.as_deref(), &sync_rule.manual_source_value);
        let should_auto_approve = is_manual && sync_rule.manual_auto_approve;

        match existing_index.get(&row.unique_key) {
            Some(existing) => {
                if should_auto_approve && !audit_result_is_approved(&existing.fields, sync_rule) {
                    need_update_records.push(UpdateRecordItem {
                        record_id: existing.record_id.clone(),
                        fields: Value::Object(approved_audit_result_fields(sync_rule)),
                    });
                }
            }
            None => {
                let mut fields = Map::new();
                fields.insert(
                    unique_key_field.to_string(),
                    rules.to_bitable_write_value(unique_key_field, Value::String(row.unique_key)),
                );

                if should_auto_approve {
                    fields.extend(approved_audit_result_fields(sync_rule));
                }

                need_create_records.push(CreateRecordItem {
                    fields: Value::Object(fields),
                });
            }
        }
    }

    (need_create_records, need_update_records)
}

/// 判断是否来自手动登记。
fn is_manual_data_source(data_source: Option<&str>, manual_source_value: &str) -> bool {
    data_source
        .map(str::trim)
        .is_some_and(|value| value == manual_source_value)
}

/// 判断 audit 表已有审核结果是否已经是审核通过。
fn audit_result_is_approved(
    fields: &Map<String, Value>,
    sync_rule: &ActivityTableSyncRuleConfig,
) -> bool {
    fields
        .iter()
        .find(|(field_name, _)| {
            normalize_field_name(field_name) == normalize_field_name(&sync_rule.audit_result_field)
        })
        .and_then(|(_, value)| value_to_plain_string(value))
        .map(|value| value == sync_rule.manual_auto_approve_result)
        .unwrap_or(false)
}

/// 构建写入 audit 表的审核通过字段。
fn approved_audit_result_fields(sync_rule: &ActivityTableSyncRuleConfig) -> Map<String, Value> {
    let mut fields = Map::new();
    fields.insert(
        sync_rule.audit_result_field.clone(),
        Value::String(sync_rule.manual_auto_approve_result.clone()),
    );
    fields
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manual_only_sync_does_not_parse_an_empty_sheet() {
        let rows = parse_spreadsheet_source_rows(
            ActivityTableSyncSourceMode::ManualOnly,
            Vec::new(),
            "视频/图文ID",
            &FieldValueRules::new(),
        )
        .expect("manual-only sync must not require Sheet headers");

        assert!(rows.is_empty());
    }

    #[test]
    fn full_sync_still_rejects_an_empty_sheet() {
        let error = parse_spreadsheet_source_rows(
            ActivityTableSyncSourceMode::SpreadsheetAndManual,
            Vec::new(),
            "视频/图文ID",
            &FieldValueRules::new(),
        )
        .expect_err("full sync still requires Sheet headers");

        assert!(format!("{error:#}").contains("Sheet 为空，缺少表头"));
    }

    #[test]
    fn manual_only_sync_recognizes_existing_spreadsheet_owned_rows() {
        let mut existing_index = HashMap::new();
        existing_index.insert(
            "xingtu-video".to_string(),
            ExistingRecordIndexItem {
                record_id: "rec_xingtu".to_string(),
                fields: Map::from_iter([(
                    "数据来源".to_string(),
                    Value::String("星图数据".to_string()),
                )]),
            },
        );
        existing_index.insert(
            "manual-video".to_string(),
            ExistingRecordIndexItem {
                record_id: "rec_manual".to_string(),
                fields: Map::from_iter([(
                    "数据来源".to_string(),
                    Value::String("手动登记".to_string()),
                )]),
            },
        );

        let keys = spreadsheet_owned_keys(&existing_index, "数据来源", "星图数据");

        assert_eq!(keys, HashSet::from(["xingtu-video".to_string()]));
    }

    #[test]
    fn default_merge_keeps_xingtu_duplicate_and_adds_manual_only_video() {
        let sync_rule = ActivityTableSyncRuleConfig::default();
        assert!(!sync_rule.manual_overrides_spreadsheet);

        let spreadsheet_rows = vec![SourceRow {
            unique_key: "shared-video".to_string(),
            fields: Map::from_iter([("播放量".to_string(), Value::from(1200))]),
        }];
        let manual_rows = vec![
            SourceRow {
                unique_key: "shared-video".to_string(),
                fields: Map::from_iter([("播放量".to_string(), Value::from(100))]),
            },
            SourceRow {
                unique_key: "manual-only-video".to_string(),
                fields: Map::new(),
            },
        ];

        let merged = merge_source_rows(
            spreadsheet_rows,
            manual_rows,
            sync_rule.manual_overrides_spreadsheet,
        );

        assert_eq!(merged.len(), 2);
        assert_eq!(merged[0].unique_key, "shared-video");
        assert_eq!(merged[0].fields["播放量"], Value::from(1200));
        assert_eq!(merged[1].unique_key, "manual-only-video");
    }

    #[test]
    fn manual_audit_row_drops_extra_source_fields_when_created() {
        let sync_rule = ActivityTableSyncRuleConfig::default();
        let source_rows = vec![SourceRow {
            unique_key: "manual-key".to_string(),
            fields: Map::from_iter([
                (
                    sync_rule.data_source_field.clone(),
                    Value::String(sync_rule.manual_source_value.clone()),
                ),
                (
                    "其他业务展示字段".to_string(),
                    Value::String("保留在主表".to_string()),
                ),
                ("播放量".to_string(), Value::from(1234)),
            ]),
        }];
        let rows = collect_audit_source_rows(&source_rows, &sync_rule.data_source_field);
        let existing_index = HashMap::new();

        let (create_records, update_records) = build_audit_sync_records(
            rows,
            &existing_index,
            "视频/图文ID",
            &FieldValueRules::new(),
            &sync_rule,
        );

        assert_eq!(create_records.len(), 1);
        assert!(update_records.is_empty());
        let fields = create_records[0].fields.as_object().unwrap();
        assert_eq!(fields.len(), 2);
        assert_eq!(fields["视频/图文ID"], "manual-key");
        assert_eq!(
            fields[&sync_rule.audit_result_field],
            sync_rule.manual_auto_approve_result
        );
        assert!(!fields.contains_key("其他业务展示字段"));
        assert!(!fields.contains_key("播放量"));
    }

    #[test]
    fn existing_manual_audit_row_drops_extra_source_fields_when_updated() {
        let sync_rule = ActivityTableSyncRuleConfig::default();
        let source_rows = vec![SourceRow {
            unique_key: "manual-key".to_string(),
            fields: Map::from_iter([
                (
                    sync_rule.data_source_field.clone(),
                    Value::String(sync_rule.manual_source_value.clone()),
                ),
                (
                    "其他业务展示字段".to_string(),
                    Value::String("保留在主表".to_string()),
                ),
            ]),
        }];
        let rows = collect_audit_source_rows(&source_rows, &sync_rule.data_source_field);
        let mut existing_index = HashMap::new();
        existing_index.insert(
            "manual-key".to_string(),
            ExistingRecordIndexItem {
                record_id: "rec_manual".to_string(),
                fields: Map::new(),
            },
        );

        let (create_records, update_records) = build_audit_sync_records(
            rows,
            &existing_index,
            "视频/图文ID",
            &FieldValueRules::new(),
            &sync_rule,
        );

        assert!(create_records.is_empty());
        assert_eq!(update_records.len(), 1);
        let fields = update_records[0].fields.as_object().unwrap();
        assert_eq!(fields.len(), 1);
        assert_eq!(
            fields[&sync_rule.audit_result_field],
            sync_rule.manual_auto_approve_result
        );
        assert!(!fields.contains_key("其他业务展示字段"));
    }

    #[test]
    fn manual_audit_row_without_auto_approval_only_writes_unique_key() {
        let sync_rule = ActivityTableSyncRuleConfig {
            manual_auto_approve: false,
            ..ActivityTableSyncRuleConfig::default()
        };
        let source_rows = vec![SourceRow {
            unique_key: "manual-key".to_string(),
            fields: Map::from_iter([
                (
                    sync_rule.data_source_field.clone(),
                    Value::String(sync_rule.manual_source_value.clone()),
                ),
                (
                    "其他业务展示字段".to_string(),
                    Value::String("保留在主表".to_string()),
                ),
            ]),
        }];
        let rows = collect_audit_source_rows(&source_rows, &sync_rule.data_source_field);

        let (create_records, update_records) = build_audit_sync_records(
            rows,
            &HashMap::new(),
            "视频/图文ID",
            &FieldValueRules::new(),
            &sync_rule,
        );

        assert_eq!(create_records.len(), 1);
        assert!(update_records.is_empty());
        let fields = create_records[0].fields.as_object().unwrap();
        assert_eq!(fields.len(), 1);
        assert_eq!(fields["视频/图文ID"], "manual-key");
    }

    #[test]
    fn xingtu_audit_row_does_not_set_audit_result() {
        let sync_rule = ActivityTableSyncRuleConfig::default();
        let rows = vec![AuditSourceRow {
            unique_key: "xingtu-key".to_string(),
            data_source: Some(sync_rule.spreadsheet_source_value.clone()),
        }];
        let existing_index = HashMap::new();

        let (create_records, update_records) = build_audit_sync_records(
            rows,
            &existing_index,
            "视频/图文ID",
            &FieldValueRules::new(),
            &sync_rule,
        );

        assert_eq!(create_records.len(), 1);
        assert!(update_records.is_empty());
        assert!(
            create_records[0]
                .fields
                .get(&sync_rule.audit_result_field)
                .is_none()
        );
    }
}
