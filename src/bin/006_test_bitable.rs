use lark_exp::lark::im::{MessageReceiver, ReceiveIdType};
use lark_exp::pipeline::activity::{
    ActivityAuditNoticeConfig, ActivityAuditTableConfig, ActivitySyncConfig,
    ActivityTableSyncConfig, ActivityTableSyncRuleConfig, ActivityTableSyncSourceMode,
    AuditNoticeWorkflowConfig, DailyMorningReviewWorkflowConfig,
};
use lark_exp::pipeline::workflow::run_daily_morning_review_workflow;
use std::env;

/// 006：手动模拟“每日早上审核工作流”触发。
///
/// 当前测试结构：
/// 1. 先按 `sync_activities` 顺序同步 13、14 两期活动
/// 2. 两期同步都完成后，再读取 audit 表统计待审核数量并发送通知
#[tokio::main]
async fn main() -> anyhow::Result<()> {
    run_daily_morning_review_workflow(build_current_workflow_config()?).await
}

/// 当前测试用的每日审核 workflow 配置。
///
/// 这里保留 13、14 两期的完整硬编码配置，方便手动填写和调试。
/// 下面几个 constructor 只负责减少 `.to_string()` 和 `Some(...)` 的重复噪音。
fn build_current_workflow_config() -> anyhow::Result<DailyMorningReviewWorkflowConfig> {
    Ok(DailyMorningReviewWorkflowConfig {
        sync_activities: vec![
            ActivitySyncConfig {
                period: "6月第十三期".to_string(),
                live: Some(table_sync_config(
                    "https://example.feishu.cn/wiki/ExampleBitableOne",
                    "https://example.larksuite.com/sheets/ExampleLiveSheetOne",
                    Some("tblExampleLiveManual1"),
                    "tblExampleLiveMain1",
                    Some("tblExampleLiveAudit1"),
                )),
                video: Some(table_sync_config(
                    "https://example.feishu.cn/wiki/ExampleBitableOne",
                    "https://example.larksuite.com/sheets/ExampleVideoSheetOne",
                    Some("tblExampleVideoManual1"),
                    "tblExampleVideoMain1",
                    Some("tblExampleVideoAudit1"),
                )),
            },
            ActivitySyncConfig {
                period: "7月第十四期".to_string(),
                live: Some(table_sync_config(
                    "https://example.feishu.cn/base/ExampleBitableTwo?table=tblExampleVideoMain2&view=vewKYQsbpI",
                    "https://example.larksuite.com/sheets/ExampleLiveSheetTwo",
                    Some("tblExampleLiveManual2"),
                    "tblExampleLiveMain2",
                    Some("tblExampleLiveAudit2"),
                )),
                video: Some(table_sync_config(
                    "https://example.feishu.cn/base/ExampleBitableTwo?table=tblExampleVideoMain2&view=vewKYQsbpI",
                    "https://example.larksuite.com/sheets/ExampleVideoSheetTwo",
                    Some("tblExampleVideoManual2"),
                    "tblExampleVideoMain2",
                    Some("tblExampleVideoAudit2"),
                )),
            },
        ],
        audit_notice: Some(AuditNoticeWorkflowConfig {
            receiver: MessageReceiver {
                receive_id_type: ReceiveIdType::ChatId,
                receive_id: "oc_aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".to_string(),
                // receive_id: "oc_bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb".to_string(),
                uuid: None,
            },
            card_template_id: required_env("AUDIT_REVIEW_CARD_TEMPLATE_ID")?,
            auditor_ids: required_env("AUDIT_REVIEW_AUDITOR_IDS")?,
            project_name: "DEMO".to_string(),
            audit_result_field: "审核结果".to_string(),
            activities: vec![
                audit_notice_activity(
                    "6月第十三期",
                    "[6月第十三期](https://example.feishu.cn/wiki/ExampleBitableOne?table=tblExampleLiveAudit1&view=vewk1qnI5t)",
                    audit_table_config(
                        "https://example.feishu.cn/wiki/ExampleBitableOne",
                        "tblExampleLiveAudit1",
                    ),
                    audit_table_config(
                        "https://example.feishu.cn/wiki/ExampleBitableOne",
                        "tblExampleVideoAudit1",
                    ),
                ),
                audit_notice_activity(
                    "7月第十四期",
                    "[7月第十四期](https://example.feishu.cn/base/ExampleBitableTwo?table=tblExampleVideoAudit2&view=vewlUrgXyU)",
                    audit_table_config(
                        "https://example.feishu.cn/base/ExampleBitableTwo",
                        "tblExampleLiveAudit2",
                    ),
                    audit_table_config(
                        "https://example.feishu.cn/base/ExampleBitableTwo",
                        "tblExampleVideoAudit2",
                    ),
                ),
            ],
        }),
    })
}

fn required_env(name: &str) -> anyhow::Result<String> {
    env::var(name)
        .map(|value| value.trim().to_string())
        .map_err(|_| anyhow::anyhow!("手动调试需要环境变量 {name}"))
        .and_then(|value| {
            anyhow::ensure!(!value.is_empty(), "环境变量 {name} 不能为空");
            Ok(value)
        })
}

/// 构造单个直播/视频同步配置。
fn table_sync_config(
    bitable_url: &str,
    spreadsheet_url: &str,
    manual_table_id: Option<&str>,
    main_table_id: &str,
    audit_table_id: Option<&str>,
) -> ActivityTableSyncConfig {
    ActivityTableSyncConfig {
        bitable_url: bitable_url.to_string(),
        spreadsheet_url: spreadsheet_url.to_string(),
        source_mode: ActivityTableSyncSourceMode::SpreadsheetAndManual,
        manual_table_id: manual_table_id.map(str::to_string),
        main_table_id: main_table_id.to_string(),
        audit_table_id: audit_table_id.map(str::to_string),
        sync_rule: ActivityTableSyncRuleConfig::default(),
        db_sync: None,
    }
}

/// 构造单个审核表统计配置。
fn audit_table_config(bitable_url: &str, audit_table_id: &str) -> ActivityAuditTableConfig {
    ActivityAuditTableConfig {
        bitable_url: bitable_url.to_string(),
        audit_table_id: audit_table_id.to_string(),
    }
}

/// 构造单期通知卡片中的统计配置。
fn audit_notice_activity(
    period: &str,
    audit_table_url: &str,
    live: ActivityAuditTableConfig,
    video: ActivityAuditTableConfig,
) -> ActivityAuditNoticeConfig {
    ActivityAuditNoticeConfig {
        period: period.to_string(),
        audit_table_url: audit_table_url.to_string(),
        live: Some(live),
        video: Some(video),
    }
}
