use crate::xingtu::account::XingtuSessionRegistry;
use crate::xingtu::activity_config::{
    ExportableContent, FeishuSourceInsertOptions, XingtuActivityConfigRepository,
};
use crate::xingtu::{ExportTaskOptions, XingtuClient};
use anyhow::{Context, anyhow};
use chrono::{DateTime, NaiveDate, Utc};
use chrono_tz::Asia::Shanghai;
use salvo::oapi::ToSchema;
use serde::Serialize;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::time::{MissedTickBehavior, interval, sleep};

const EXPORT_TASK_JITTER_MAX_SECS: u64 = 15;

/// 星图拉取触发配置。
#[derive(Debug, Clone)]
pub struct XingtuTraceRunOptions {
    pub export_options: ExportTaskOptions,
    pub activity_period_id: Option<i64>,
    pub jitter_before_first_export: bool,
    pub trigger_type: String,
    pub is_daily_final: bool,
    pub pull_started_at: DateTime<Utc>,
    pub created_by: String,
}

impl Default for XingtuTraceRunOptions {
    fn default() -> Self {
        let pull_started_at = Utc::now();

        Self {
            export_options: ExportTaskOptions::default(),
            activity_period_id: None,
            jitter_before_first_export: false,
            trigger_type: "periodic".to_string(),
            is_daily_final: false,
            pull_started_at,
            created_by: "xingtu_trace".to_string(),
        }
    }
}

/// 单个内容配置的拉取结果。
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct TraceContentResult {
    pub content_config_id: i64,
    pub feishu_source_id: i64,
    pub project: String,
    pub period: String,
    pub xingtu_account_id: String,
    pub content_type: String,
    pub xingtu_task_id: String,
    pub spreadsheet_url: String,
    pub stat_date: chrono::NaiveDate,
    pub is_daily_final: bool,
    pub changed: bool,
    pub ticket_id: String,
    pub polls: u64,
}

/// 一轮定时拉取结果。
#[derive(Debug, Clone, Default, Serialize, ToSchema)]
pub struct TraceRoundResult {
    pub traced_count: usize,
    pub skipped_count: usize,
    pub results: Vec<TraceContentResult>,
}

/// 运行一轮星图源 Sheet 链接拉取。
///
/// 这个函数只负责“拉取链接并回写数据库”，不直接同步业务主表。
/// 后续调度器可以在本函数成功后继续触发直播/视频同步 pipeline。
pub async fn run_xingtu_trace_once(
    repo: &XingtuActivityConfigRepository,
    xingtu: &XingtuClient,
    options: ExportTaskOptions,
) -> anyhow::Result<TraceRoundResult> {
    let pull_started_at = Utc::now();
    let contents = repo.list_exportable_contents(pull_started_at, None).await?;

    if contents.is_empty() {
        tracing::info!("没有需要拉取的星图内容配置，本轮跳过");
        return Ok(TraceRoundResult::default());
    }

    let mut round = TraceRoundResult {
        traced_count: contents.len(),
        skipped_count: 0,
        results: Vec::new(),
    };

    for (index, content) in contents.into_iter().enumerate() {
        sleep_between_export_tasks(index, false).await;
        let error_context = trace_error_context(&content);
        let run_options = XingtuTraceRunOptions {
            export_options: options.clone(),
            activity_period_id: Some(content.activity_period_id),
            jitter_before_first_export: false,
            trigger_type: "periodic".to_string(),
            is_daily_final: false,
            pull_started_at,
            created_by: "xingtu_trace".to_string(),
        };
        let result = trace_single_content(repo, xingtu, content, run_options)
            .await
            .with_context(|| error_context)?;

        round.results.push(result);
    }

    Ok(round)
}

/// 使用账号注册表运行一轮星图源 Sheet 链接拉取。
///
/// 服务化后每个项目可能使用不同星图账号，因此这里按内容配置中的
/// `xingtu_account_id` 选择对应登录态。
pub async fn run_xingtu_trace_once_with_registry(
    repo: &XingtuActivityConfigRepository,
    registry: &XingtuSessionRegistry,
    options: XingtuTraceRunOptions,
) -> anyhow::Result<TraceRoundResult> {
    let contents = repo
        .list_exportable_contents(options.pull_started_at, options.activity_period_id)
        .await?;

    if contents.is_empty() {
        tracing::info!("没有需要拉取的星图内容配置，本轮跳过");
        return Ok(TraceRoundResult::default());
    }

    let mut round = TraceRoundResult {
        traced_count: contents.len(),
        skipped_count: 0,
        results: Vec::new(),
    };

    for (index, content) in contents.into_iter().enumerate() {
        sleep_between_export_tasks(index, options.jitter_before_first_export).await;
        let error_context = trace_error_context(&content);
        let xingtu = registry
            .get_client(&content.xingtu_account_id)
            .await
            .with_context(|| {
                format!(
                    "获取星图账号登录态失败：{} {}",
                    content.period, content.xingtu_account_id
                )
            })?;

        let result = trace_single_content(repo, &xingtu, content, options.clone())
            .await
            .with_context(|| error_context)?;

        round.results.push(result);
    }

    Ok(round)
}

fn trace_error_context(content: &ExportableContent) -> String {
    format!(
        "拉取星图内容失败：project={} period={} account_id={} content_type={} task_id={}",
        content.project,
        content.period,
        content.xingtu_account_id,
        content.content_type.as_db_value(),
        content.xingtu_task_id
    )
}

/// 按固定间隔循环运行星图拉取。
///
/// 实验阶段可传 5 分钟；生产后建议由外部调度器控制频率，
/// 或者把这个循环放到独立 worker 中。
pub async fn run_xingtu_trace_loop(
    repo: XingtuActivityConfigRepository,
    xingtu: XingtuClient,
    options: ExportTaskOptions,
    interval_duration: Duration,
) -> anyhow::Result<()> {
    if interval_duration.is_zero() {
        return Err(anyhow!("定时拉取间隔不能为 0"));
    }

    let mut ticker = interval(interval_duration);
    ticker.set_missed_tick_behavior(MissedTickBehavior::Delay);

    loop {
        ticker.tick().await;
        tracing::info!("开始执行星图定时拉取，本轮间隔 {:?}", interval_duration);

        match run_xingtu_trace_once(&repo, &xingtu, options.clone()).await {
            Ok(result) => {
                tracing::info!(
                    "星图定时拉取完成：traced={} skipped={} updated={}",
                    result.traced_count,
                    result.skipped_count,
                    result.results.len()
                );
            }
            Err(error) => {
                tracing::error!("星图定时拉取失败：{error:?}");
            }
        }
    }
}

async fn trace_single_content(
    repo: &XingtuActivityConfigRepository,
    xingtu: &XingtuClient,
    content: ExportableContent,
    options: XingtuTraceRunOptions,
) -> anyhow::Result<TraceContentResult> {
    tracing::info!(
        "开始拉取星图任务：{} {} {} account_id={} task_id={}",
        content.project,
        content.period,
        content.content_type.as_db_value(),
        content.xingtu_account_id,
        content.xingtu_task_id
    );

    let export = xingtu
        .export_task_spreadsheet_urls(&content.xingtu_task_id, options.export_options.clone())
        .await
        .with_context(|| format!("星图任务导出失败：{}", content.xingtu_task_id))?;

    let spreadsheet_url = export
        .spreadsheet_urls
        .first()
        .cloned()
        .ok_or_else(|| anyhow!("星图导出成功但没有返回 spreadsheet_url"))?;

    let changed = content
        .previous_spreadsheet_url
        .as_deref()
        .is_none_or(|previous| previous != spreadsheet_url);

    repo.update_source_spreadsheet_url(content.content_config_id, &spreadsheet_url)
        .await?;
    let stat_date = stat_date_from_pull_started_at(options.pull_started_at);
    let feishu_source_id = repo
        .insert_feishu_source(
            &content,
            &spreadsheet_url,
            FeishuSourceInsertOptions {
                trigger_type: &options.trigger_type,
                stat_date,
                pull_started_at: options.pull_started_at,
                is_daily_final: options.is_daily_final,
                created_by: &options.created_by,
            },
        )
        .await?;

    tracing::info!(
        "星图任务拉取完成：{} {} url_changed={}",
        content.period,
        content.content_type.as_db_value(),
        changed
    );

    Ok(TraceContentResult {
        content_config_id: content.content_config_id,
        feishu_source_id,
        project: content.project,
        period: content.period,
        xingtu_account_id: content.xingtu_account_id,
        content_type: content.content_type.as_db_value().to_string(),
        xingtu_task_id: content.xingtu_task_id,
        spreadsheet_url,
        stat_date,
        is_daily_final: options.is_daily_final,
        changed,
        ticket_id: export.ticket_id,
        polls: export.polls,
    })
}

/// 根据拉取开始时间计算数据归属日期。
///
/// 23:59 发起拉取但次日才拿到 URL 时，仍然归属拉取开始当天。
fn stat_date_from_pull_started_at(pull_started_at: DateTime<Utc>) -> NaiveDate {
    pull_started_at.with_timezone(&Shanghai).date_naive()
}

async fn sleep_between_export_tasks(index: usize, jitter_before_first_export: bool) {
    if !should_jitter_before_export(index, jitter_before_first_export) {
        return;
    }

    let secs = random_export_task_jitter_secs();
    tracing::info!("星图导出任务错峰等待：{} 秒", secs);
    if secs > 0 {
        sleep(Duration::from_secs(secs)).await;
    }
}

fn should_jitter_before_export(index: usize, jitter_before_first_export: bool) -> bool {
    index > 0 || jitter_before_first_export
}

fn random_export_task_jitter_secs() -> u64 {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.subsec_nanos())
        .unwrap_or_default();
    export_task_jitter_secs_from_nanos(nanos)
}

fn export_task_jitter_secs_from_nanos(nanos: u32) -> u64 {
    u64::from(nanos) % (EXPORT_TASK_JITTER_MAX_SECS + 1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn export_task_jitter_is_zero_to_fifteen_seconds() {
        assert_eq!(export_task_jitter_secs_from_nanos(0), 0);
        assert_eq!(export_task_jitter_secs_from_nanos(15), 15);
        assert_eq!(export_task_jitter_secs_from_nanos(16), 0);
    }

    #[test]
    fn export_task_jitter_keeps_cross_project_spacing() {
        assert!(!should_jitter_before_export(0, false));
        assert!(should_jitter_before_export(1, false));
        assert!(should_jitter_before_export(0, true));
    }

    #[test]
    fn trace_error_context_keeps_project_account_and_task() {
        let context = trace_error_context(&ExportableContent {
            content_config_id: 1,
            activity_period_id: 1,
            project: "ROK".to_string(),
            period: "2026年7月第十四期".to_string(),
            xingtu_account_id: "demo-xingtu-account".to_string(),
            content_type: crate::xingtu::activity_config::XingtuContentType::Video,
            xingtu_task_id: "demo-video-task-id".to_string(),
            xingtu_task_name: None,
            previous_spreadsheet_url: None,
            trace_enabled: true,
        });

        assert!(context.contains("project=ROK"));
        assert!(context.contains("account_id=demo-xingtu-account"));
        assert!(context.contains("task_id=demo-video-task-id"));
    }
}
