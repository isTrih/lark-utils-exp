use crate::client::LarkClient;
use crate::config::Config;
use crate::pipeline::activity::{
    ActivitySyncConfig, AuditNoticeWorkflowConfig, DailyMorningReviewWorkflowConfig,
};
use crate::pipeline::audit_notice::run_audit_notice_workflow;
use crate::pipeline::live::run_live_sync;
use crate::pipeline::video::run_video_sync;
use anyhow::Context;

/// 每日早上审核工作流。
///
/// 这是给 006 调试程序使用的低层入口：调用方已经传入准备好的活动同步配置。
/// 生产环境完整的“星图导出、来源入库、手动登记合并、表同步、通知”由
/// `XingtuWorkflowService` 负责，避免在这里维护第二套任务编排。
pub async fn run_daily_morning_review_workflow(
    workflow_config: DailyMorningReviewWorkflowConfig,
) -> anyhow::Result<()> {
    tracing::info!("开始执行每日早上审核工作流");

    let app_config = Config::from_env().context("读取环境配置失败")?;
    let lark = LarkClient::new(app_config).context("创建 LarkClient 失败")?;
    let DailyMorningReviewWorkflowConfig {
        sync_activities,
        audit_notice,
    } = workflow_config;

    run_sync_data_step(lark.clone(), sync_activities).await?;
    run_audit_notice_step(&lark, audit_notice).await?;

    tracing::info!("每日早上审核工作流完成");
    Ok(())
}

/// 每 N 小时同步工作流。
///
/// 这是调试用低层入口；生产环境调度由 `server::scheduler` 和
/// `XingtuWorkflowService` 执行完整链路。`interval_hours` 只用于调试日志。
pub async fn run_periodic_sync_workflow(
    activity_configs: Vec<ActivitySyncConfig>,
    interval_hours: u64,
) -> anyhow::Result<()> {
    tracing::info!("开始执行每 {} 小时同步工作流", interval_hours);

    let app_config = Config::from_env().context("读取环境配置失败")?;
    let lark = LarkClient::new(app_config).context("创建 LarkClient 失败")?;

    run_sync_data_step(lark, activity_configs).await?;

    tracing::info!("每 {} 小时同步工作流完成", interval_hours);
    Ok(())
}

/// 直接执行一组活动同步配置。
///
/// 这个入口给调度器或测试工具使用：外层已经决定了触发时机，
/// 这里只负责创建飞书客户端并运行直播/视频同步 pipeline。
pub async fn run_activity_sync_workflow(
    activity_configs: Vec<ActivitySyncConfig>,
) -> anyhow::Result<()> {
    tracing::info!("开始执行活动同步工作流");

    let app_config = Config::from_env().context("读取环境配置失败")?;
    let lark = LarkClient::new(app_config).context("创建 LarkClient 失败")?;
    run_sync_data_step(lark, activity_configs).await?;

    tracing::info!("活动同步工作流完成");
    Ok(())
}

/// 工作流中的“同步数据”步骤。
///
/// 这里按业务 pipeline 顺序触发直播和视频同步。后续如果需要并发、重试或任务状态记录，
/// 可以在这一层增强，而不需要改视频/直播内部同步规则。
async fn run_sync_data_step(
    lark: LarkClient,
    activity_configs: Vec<ActivitySyncConfig>,
) -> anyhow::Result<()> {
    if activity_configs.is_empty() {
        tracing::info!("未配置活动同步，本次跳过同步 pipeline");
        return Ok(());
    }

    for (index, activity_config) in activity_configs.into_iter().enumerate() {
        let display_name = activity_display_name(index, &activity_config.period);

        tracing::info!("开始同步活动：{}", display_name);

        run_single_activity_sync(lark.clone(), activity_config)
            .await
            .with_context(|| format!("活动 `{}` 同步失败", display_name))?;

        tracing::info!("活动同步完成：{}", display_name);
    }

    Ok(())
}

/// 同步单期活动的数据。
async fn run_single_activity_sync(
    lark: LarkClient,
    activity_config: ActivitySyncConfig,
) -> anyhow::Result<()> {
    let ActivitySyncConfig { live, video, .. } = activity_config;

    if let Some(live_config) = live {
        run_live_sync(lark.clone(), live_config)
            .await
            .context("直播同步失败")?;
    } else {
        tracing::info!("未配置直播同步，本次跳过直播 pipeline");
    }

    if let Some(video_config) = video {
        run_video_sync(lark.clone(), video_config)
            .await
            .context("视频同步失败")?;
    } else {
        tracing::info!("未配置视频同步，本次跳过视频 pipeline");
    }

    Ok(())
}

/// 构造日志里展示的活动名。
fn activity_display_name(index: usize, period: &str) -> String {
    let period = period.trim();

    if period.is_empty() {
        format!("第 {} 期活动", index + 1)
    } else {
        period.to_string()
    }
}

/// 工作流中的“通知审核”步骤。
///
/// 未配置通知时直接跳过，方便后续复用同一套工作流做只同步的手动入口。
async fn run_audit_notice_step(
    lark: &LarkClient,
    audit_notice: Option<AuditNoticeWorkflowConfig>,
) -> anyhow::Result<()> {
    let Some(audit_notice) = audit_notice else {
        tracing::info!("未配置审核通知，本次跳过通知发送");
        return Ok(());
    };

    run_audit_notice_workflow(lark, audit_notice)
        .await
        .context("审核通知流程失败")?;

    Ok(())
}
