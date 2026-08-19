use crate::client::LarkClient;
use crate::lark::im::{AuditInfoItem, AuditReviewNoticeConfig, FeishuImClient, SendMessageResult};
use crate::lark::sheets::get_spreadsheet_token;
use crate::pipeline::activity::{
    ActivityAuditNoticeConfig, ActivityAuditTableConfig, AuditNoticeWorkflowConfig,
};
use crate::pipeline::bitable::read_optional_records_with_retry;
use crate::pipeline::sync::{normalize_field_name, value_to_plain_string};
use anyhow::Context;
use open_lark::docs::base::bitable::Record as BitableRecord;
use serde_json::{Map, Value};

/// 执行审核通知 workflow。
///
/// 流程：
/// 1. 读取配置中的各期直播/视频 audit 表
/// 2. 统计 `审核结果` 为空的待审核记录数
/// 3. 只把存在待审核记录的期数组装进卡片参数
/// 4. 如果没有任何待审核记录，则跳过发消息
pub async fn run_audit_notice_workflow(
    lark: &LarkClient,
    config: AuditNoticeWorkflowConfig,
) -> anyhow::Result<Option<SendMessageResult>> {
    tracing::info!("开始执行审核通知 workflow");

    if config.auditor_ids.trim().is_empty() {
        tracing::info!("没有启用的项目审核人，跳过审核通知发送");
        return Ok(None);
    }

    let audit_info = build_pending_audit_info(lark, &config)
        .await
        .context("构建审核通知卡片参数失败")?;

    if audit_info.is_empty() {
        tracing::info!("没有待审核记录，跳过审核通知发送");
        return Ok(None);
    }

    let notice_config = AuditReviewNoticeConfig::with_template(
        config.card_template_id.trim(),
        config.auditor_ids.trim(),
        config.project_name.trim(),
        audit_info,
    );
    let result = FeishuImClient::new(lark)
        .send_audit_review_notice(&config.receiver, &notice_config)
        .await
        .context("发送审核通知失败")?;

    tracing::info!("审核通知发送完成，message_id={:?}", result.message_id);
    Ok(Some(result))
}

/// 读取各期审核表并构建卡片模板中的 `audit_info`。
pub async fn build_pending_audit_info(
    lark: &LarkClient,
    config: &AuditNoticeWorkflowConfig,
) -> anyhow::Result<Vec<AuditInfoItem>> {
    let mut result = Vec::new();

    for activity in &config.activities {
        let live_nums = count_pending_for_optional_table(
            lark,
            activity.live.as_ref(),
            &config.audit_result_field,
        )
        .await
        .with_context(|| format!("统计 `{}` 直播待审核数量失败", activity.period))?;
        let video_nums = count_pending_for_optional_table(
            lark,
            activity.video.as_ref(),
            &config.audit_result_field,
        )
        .await
        .with_context(|| format!("统计 `{}` 视频待审核数量失败", activity.period))?;

        tracing::info!(
            "{} 待审核数量：video_nums={} live_nums={}",
            activity.period,
            video_nums,
            live_nums
        );

        if live_nums == 0 && video_nums == 0 {
            tracing::info!("{} 没有待审核记录，跳过卡片参数", activity.period);
            continue;
        }

        result.push(build_audit_info_item(activity, video_nums, live_nums));
    }

    Ok(result)
}

/// 统计可选审核表中的待审核数量。
async fn count_pending_for_optional_table(
    lark: &LarkClient,
    table_config: Option<&ActivityAuditTableConfig>,
    audit_result_field: &str,
) -> anyhow::Result<usize> {
    let Some(table_config) = table_config else {
        return Ok(0);
    };

    count_pending_for_table(lark, table_config, audit_result_field).await
}

/// 统计单张审核表中审核结果为空的记录数。
async fn count_pending_for_table(
    lark: &LarkClient,
    table_config: &ActivityAuditTableConfig,
    audit_result_field: &str,
) -> anyhow::Result<usize> {
    let app_token =
        get_spreadsheet_token(&table_config.bitable_url).context("解析多维表格 app_token 失败")?;
    let records = read_optional_records_with_retry(lark, &app_token, &table_config.audit_table_id)
        .await
        .context("读取审核表记录失败")?;

    Ok(count_pending_audit_records(&records, audit_result_field))
}

/// 按卡片模板字段构建单期审核信息。
fn build_audit_info_item(
    activity: &ActivityAuditNoticeConfig,
    video_nums: usize,
    live_nums: usize,
) -> AuditInfoItem {
    let audit_table_url = markdown_link(&activity.period, &activity.audit_table_url);

    AuditInfoItem::new(&activity.period, video_nums, live_nums, audit_table_url)
}

/// 飞书卡片模板中的链接变量需要显式传入 markdown 链接。
///
/// 数据库配置里保存纯 `bitable_url`，发送通知时再转换为 `[期次](url)`；
/// 如果调试代码已经传入 markdown 链接，则保持原样，避免重复包裹。
fn markdown_link(label: &str, url_or_markdown: &str) -> String {
    let value = url_or_markdown.trim();

    if is_markdown_link(value) {
        return value.to_string();
    }

    format!("[{}]({})", escape_markdown_link_label(label), value)
}

fn is_markdown_link(value: &str) -> bool {
    value.starts_with('[') && value.contains("](") && value.ends_with(')')
}

fn escape_markdown_link_label(value: &str) -> String {
    value.replace('[', "\\[").replace(']', "\\]")
}

/// 统计审核结果为空的记录数。
///
/// 飞书空字段可能表现为字段缺失、null、空字符串或空数组；
/// 这些情况都按“待审核”处理。
pub(crate) fn count_pending_audit_records(
    records: &[BitableRecord],
    audit_result_field: &str,
) -> usize {
    records
        .iter()
        .filter(|record| is_pending_audit_record(record, audit_result_field))
        .count()
}

/// 判断单条记录是否待审核。
fn is_pending_audit_record(record: &BitableRecord, audit_result_field: &str) -> bool {
    let Some(fields) = record.fields.as_object() else {
        return true;
    };

    match find_field_value_by_name(fields, audit_result_field) {
        Some(value) => value_to_plain_string(value).is_none(),
        None => true,
    }
}

/// 用归一化字段名读取字段值，兼容飞书字段名中的空格或换行。
fn find_field_value_by_name<'a>(
    fields: &'a Map<String, Value>,
    field_name: &str,
) -> Option<&'a Value> {
    let target = normalize_field_name(field_name);

    fields
        .iter()
        .find(|(name, _)| normalize_field_name(name) == target)
        .map(|(_, value)| value)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn count_pending_audit_records_treats_empty_values_as_pending() {
        let records = vec![
            new_record(json!({ "审核结果": "" })),
            new_record(json!({ "审核结果": [] })),
            new_record(json!({ "标题": "没有审核字段" })),
            new_record(json!({ "审核结果": [{ "text": "通过", "type": "text" }] })),
        ];

        assert_eq!(count_pending_audit_records(&records, "审核结果"), 3);
    }

    #[test]
    fn count_pending_audit_records_normalizes_field_name() {
        let records = vec![
            new_record(json!({ "审核 结果": "" })),
            new_record(json!({ "审核\n结果": "已处理" })),
        ];

        assert_eq!(count_pending_audit_records(&records, "审核结果"), 1);
    }

    #[test]
    fn build_audit_info_item_wraps_plain_url_as_markdown_link() {
        let activity = ActivityAuditNoticeConfig {
            period: "2026年7月第十四期".to_string(),
            audit_table_url: "https://example.com/base/table".to_string(),
            live: None,
            video: None,
        };

        let item = build_audit_info_item(&activity, 12, 3);

        assert_eq!(
            item.audit_table_url,
            "[2026年7月第十四期](https://example.com/base/table)"
        );
    }

    #[test]
    fn build_audit_info_item_keeps_existing_markdown_link() {
        let activity = ActivityAuditNoticeConfig {
            period: "2026年7月第十四期".to_string(),
            audit_table_url: "[已有链接](https://example.com/base/table)".to_string(),
            live: None,
            video: None,
        };

        let item = build_audit_info_item(&activity, 12, 3);

        assert_eq!(
            item.audit_table_url,
            "[已有链接](https://example.com/base/table)"
        );
    }

    fn new_record(fields: Value) -> BitableRecord {
        BitableRecord {
            record_id: "rec_test".to_string(),
            fields,
            created_by: None,
            created_time: None,
            last_modified_by: None,
            last_modified_time: None,
            shared_url: None,
            record_url: None,
        }
    }
}
