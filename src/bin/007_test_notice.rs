use lark_exp::client::LarkClient;
use lark_exp::config::Config;
use lark_exp::lark::im::{MessageReceiver, ReceiveIdType};
use lark_exp::pipeline::activity::{
    ActivityAuditNoticeConfig, ActivityAuditTableConfig, AuditNoticeWorkflowConfig,
};
use lark_exp::pipeline::audit_notice::run_audit_notice_workflow;
use std::env;

/// 007：手动模拟“审核通知 workflow”触发。
///
/// 后续接入任务调度器后，这里的 `build_current_notice_config`
/// 会由后端任务配置替代；workflow 会自己读取 audit 表并计算待审核数量。
#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let config = Config::from_env()?;
    let lark = LarkClient::new(config)?;
    let notice_config = build_current_notice_config()?;

    let result = run_audit_notice_workflow(&lark, notice_config).await?;

    match result {
        Some(result) => {
            println!("审核通知发送完成，message_id={:?}", result.message_id);
            println!("飞书原始响应：{}", result.raw_response);
        }
        None => println!("当前没有待审核数据，本次未发送审核通知"),
    }

    Ok(())
}

/// 当前测试用的通知配置。
///
/// 这里模拟调度器读取配置后传入方法，不从环境变量读取接收者。
/// 实际接入企业后端时，每期活动的 audit 表配置应来自任务配置。
fn build_current_notice_config() -> anyhow::Result<AuditNoticeWorkflowConfig> {
    let bitable_url = "https://example.feishu.cn/base/ExampleBitableTwo";

    Ok(AuditNoticeWorkflowConfig::new(
        MessageReceiver {
            receive_id_type: ReceiveIdType::ChatId,
            receive_id: "oc_aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".to_string(),
            uuid: None,
        },
        required_env("AUDIT_REVIEW_CARD_TEMPLATE_ID")?,
        required_env("AUDIT_REVIEW_AUDITOR_IDS")?,
        "DEMO",
        vec![ActivityAuditNoticeConfig {
            period: "6月第十三期".to_string(),
            audit_table_url: "[6月第十三期](https://example.feishu.cn/base/ExampleBitableTwo?table=tblExampleVideoAudit2&view=vewlUrgXyU)".to_string(),
            live: Some(ActivityAuditTableConfig {
                bitable_url: bitable_url.to_string(),
                audit_table_id: "tblExampleLiveAudit2".to_string(),
            }),
            video: Some(ActivityAuditTableConfig {
                bitable_url: bitable_url.to_string(),
                audit_table_id: "tblExampleVideoAudit2".to_string(),
            }),
        }],
    ))
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
