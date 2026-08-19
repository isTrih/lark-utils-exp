use crate::client::LarkClient;
use crate::lark::sheets::get_spreadsheet_token;
use crate::pipeline::activity::ActivityAuditResultSyncConfig;
use crate::pipeline::bitable::read_existing_records_with_retry;
use crate::pipeline::sync::{find_field_value_by_name, value_to_plain_string};
use crate::xingtu::data_import::{AuditResultUpdate, XingtuDataImportRepository};
use anyhow::{Context, anyhow};
use open_lark::docs::base::bitable::Record as BitableRecord;
use salvo::oapi::ToSchema;
use serde::Serialize;
use serde_json::{Map, Value};
use std::collections::BTreeMap;

const VIDEO_AUDIT_LABEL_FIELD: &str = "审核标签";
const AUDIT_EXTRA_FIELD_PREFIX: &str = "【额外】";

/// 一次审核结果回写的汇总。
#[derive(Debug, Clone, Default, Serialize, ToSchema)]
pub struct AuditResultSyncResult {
    pub tables_processed: usize,
    pub records_read: usize,
    pub reviewed_records: usize,
    pub database_rows_updated: usize,
}

/// 读取活动审核表中已经填写的审核结果，并回写视频/直播业务表。
pub async fn sync_audit_results_to_database(
    lark: &LarkClient,
    repository: &XingtuDataImportRepository,
    configs: Vec<ActivityAuditResultSyncConfig>,
) -> anyhow::Result<AuditResultSyncResult> {
    let mut summary = AuditResultSyncResult::default();

    for config in configs {
        let content_type = config.content_type.as_db_value();
        let display_name = format!("{} {}", config.period, content_type);
        let app_token = get_spreadsheet_token(&config.bitable_url)
            .with_context(|| format!("解析活动 `{display_name}` 多维表格 app_token 失败"))?;
        let records = read_existing_records_with_retry(lark, &app_token, &config.audit_table_id)
            .await
            .with_context(|| format!("读取活动 `{display_name}` 审核表失败"))?;
        let updates = collect_reviewed_audit_results(
            &records,
            config.content_type,
            unique_key_field(config.content_type),
            &config.audit_result_field,
        )
        .with_context(|| format!("解析活动 `{display_name}` 审核结果失败"))?;
        let updated = repository
            .update_audit_results(config.content_config_id, content_type, &updates)
            .await
            .with_context(|| format!("回写活动 `{display_name}` 审核结果失败"))?;

        summary.tables_processed += 1;
        summary.records_read += records.len();
        summary.reviewed_records += updates.len();
        summary.database_rows_updated += updated;

        tracing::info!(
            "审核结果同步完成：activity={} table_id={} records={} reviewed={} updated={}",
            display_name,
            config.audit_table_id,
            records.len(),
            updates.len(),
            updated
        );
    }

    Ok(summary)
}

fn unique_key_field(
    content_type: crate::xingtu::activity_config::XingtuContentType,
) -> &'static str {
    use crate::pipeline::live::LIVE_UNIQUE_KEY_FIELD;
    use crate::pipeline::video::VIDEO_UNIQUE_KEY_FIELD;
    use crate::xingtu::activity_config::XingtuContentType;

    match content_type {
        XingtuContentType::Live => LIVE_UNIQUE_KEY_FIELD,
        XingtuContentType::Video => VIDEO_UNIQUE_KEY_FIELD,
    }
}

fn collect_reviewed_audit_results(
    records: &[BitableRecord],
    content_type: crate::xingtu::activity_config::XingtuContentType,
    unique_key_field: &str,
    audit_result_field: &str,
) -> anyhow::Result<Vec<AuditResultUpdate>> {
    let mut updates = BTreeMap::new();

    for record in records {
        let Some(fields) = record.fields.as_object() else {
            continue;
        };
        let Some(unique_key) = find_field_value_by_name(fields, unique_key_field)
            .and_then(value_to_plain_string)
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
        else {
            continue;
        };
        let Some(audit_result) = find_field_value_by_name(fields, audit_result_field)
            .and_then(value_to_plain_string)
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
        else {
            continue;
        };
        let label = if matches!(
            content_type,
            crate::xingtu::activity_config::XingtuContentType::Video
        ) {
            find_field_value_by_name(fields, VIDEO_AUDIT_LABEL_FIELD)
                .and_then(value_to_plain_string)
                .map(|value| value.trim().to_string())
                .filter(|value| !value.is_empty())
        } else {
            None
        };

        let update = AuditResultUpdate {
            unique_key: unique_key.clone(),
            audit_result,
            label,
            audit_extra: collect_audit_extra(fields)
                .with_context(|| format!("审核表唯一键 `{unique_key}` 的额外审核字段不合法"))?,
        };

        if let Some(previous) = updates.insert(unique_key.clone(), update.clone())
            && previous != update
        {
            return Err(anyhow!(
                "审核表唯一键 `{unique_key}` 存在冲突审核数据：\
                     result=`{}` label=`{:?}` 与 result=`{}` label=`{:?}`；额外字段也必须一致",
                previous.audit_result,
                previous.label,
                update.audit_result,
                update.label
            ));
        }
    }

    Ok(updates.into_values().collect())
}

fn collect_audit_extra(fields: &Map<String, Value>) -> anyhow::Result<Value> {
    let mut audit_extra = Map::new();

    for (field_name, value) in fields {
        let field_name = field_name.trim();
        let Some(extra_name) = field_name.strip_prefix(AUDIT_EXTRA_FIELD_PREFIX) else {
            continue;
        };
        let extra_name = extra_name.trim();
        if extra_name.is_empty() {
            return Err(anyhow!(
                "字段名 `{field_name}` 缺少“【额外】”后的 JSON 键名"
            ));
        }

        let value = normalize_audit_extra_value(value);
        if let Some(previous) = audit_extra.insert(extra_name.to_owned(), value.clone())
            && previous != value
        {
            return Err(anyhow!(
                "字段名去掉“【额外】”前缀后产生重复键 `{extra_name}`"
            ));
        }
    }

    Ok(Value::Object(audit_extra))
}

fn normalize_audit_extra_value(value: &Value) -> Value {
    match value {
        Value::Object(object)
            if object.get("type").is_some_and(Value::is_number) && object.contains_key("value") =>
        {
            // 飞书文本、数字、布尔等字段可能额外包一层 `{ type, value }`。
            // 这层是传输结构，不属于业务值；递归解包后仍保留原 JSON 类型。
            normalize_audit_extra_value(&object["value"])
        }
        Value::Array(items) if !items.is_empty() && items.iter().all(is_feishu_text_segment) => {
            let text = items
                .iter()
                .filter_map(|item| item.get("text").and_then(Value::as_str))
                .collect::<String>();
            Value::String(text)
        }
        Value::Object(object) if is_feishu_text_segment(value) => object
            .get("text")
            .and_then(Value::as_str)
            .map(|text| Value::String(text.to_owned()))
            .unwrap_or_else(|| value.clone()),
        _ => value.clone(),
    }
}

fn is_feishu_text_segment(value: &Value) -> bool {
    value.as_object().is_some_and(|object| {
        object.get("text").is_some_and(Value::is_string)
            && object
                .get("type")
                .and_then(Value::as_str)
                .is_none_or(|value| value == "text")
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Value, json};

    #[test]
    fn collects_only_non_empty_review_results() {
        use crate::xingtu::activity_config::XingtuContentType;

        let records = vec![
            new_record(json!({
                "视频/图文 ID": "video-1",
                "审核 结果": "审核通过",
                "审核 标签": "重点"
            })),
            new_record(json!({
                "视频/图文ID": "video-2",
                "审核结果": "",
                "审核标签": "不应同步"
            })),
            new_record(json!({
                "视频/图文ID": "video-3",
                "审核结果": [{ "text": "不通过", "type": "text" }],
                "审核标签": [{ "text": "风险", "type": "text" }]
            })),
        ];

        let updates = collect_reviewed_audit_results(
            &records,
            XingtuContentType::Video,
            "视频/图文ID",
            "审核结果",
        )
        .expect("audit results should parse");

        assert_eq!(
            updates,
            vec![
                AuditResultUpdate {
                    unique_key: "video-1".to_string(),
                    audit_result: "审核通过".to_string(),
                    label: Some("重点".to_string()),
                    audit_extra: json!({}),
                },
                AuditResultUpdate {
                    unique_key: "video-3".to_string(),
                    audit_result: "不通过".to_string(),
                    label: Some("风险".to_string()),
                    audit_extra: json!({}),
                },
            ]
        );
    }

    #[test]
    fn collects_empty_or_missing_video_label_as_none() {
        use crate::xingtu::activity_config::XingtuContentType;

        let records = vec![
            new_record(json!({
                "视频/图文ID": "video-1",
                "审核结果": "审核通过",
                "审核标签": ""
            })),
            new_record(json!({
                "视频/图文ID": "video-2",
                "审核结果": "审核通过"
            })),
        ];

        let updates = collect_reviewed_audit_results(
            &records,
            XingtuContentType::Video,
            "视频/图文ID",
            "审核结果",
        )
        .expect("empty labels should clear the database label");

        assert_eq!(updates.len(), 2);
        assert!(updates.iter().all(|update| update.label.is_none()));
    }

    #[test]
    fn rejects_conflicting_results_for_same_unique_key() {
        use crate::xingtu::activity_config::XingtuContentType;

        let records = vec![
            new_record(json!({ "直播间ID": "live-1", "审核结果": "通过" })),
            new_record(json!({ "直播间ID": "live-1", "审核结果": "不通过" })),
        ];

        assert!(
            collect_reviewed_audit_results(
                &records,
                XingtuContentType::Live,
                "直播间ID",
                "审核结果"
            )
            .is_err()
        );
    }

    #[test]
    fn rejects_conflicting_video_labels_for_same_unique_key() {
        use crate::xingtu::activity_config::XingtuContentType;

        let records = vec![
            new_record(json!({
                "视频/图文ID": "video-1",
                "审核结果": "通过",
                "审核标签": "A"
            })),
            new_record(json!({
                "视频/图文ID": "video-1",
                "审核结果": "通过",
                "审核标签": "B"
            })),
        ];

        assert!(
            collect_reviewed_audit_results(
                &records,
                XingtuContentType::Video,
                "视频/图文ID",
                "审核结果"
            )
            .is_err()
        );
    }

    #[test]
    fn live_audit_results_ignore_video_label_field() {
        use crate::xingtu::activity_config::XingtuContentType;

        let records = vec![new_record(json!({
            "直播间ID": "live-1",
            "审核结果": "通过",
            "审核标签": "不应同步",
            "【额外】直播推荐": true
        }))];

        let updates = collect_reviewed_audit_results(
            &records,
            XingtuContentType::Live,
            "直播间ID",
            "审核结果",
        )
        .expect("live audit results should parse");

        assert_eq!(updates[0].label, None);
        assert_eq!(updates[0].audit_extra, json!({ "直播推荐": true }));
    }

    #[test]
    fn collects_audit_extra_fields_and_preserves_json_types() {
        use crate::xingtu::activity_config::XingtuContentType;

        let records = vec![new_record(json!({
            "视频/图文ID": "video-extra",
            "审核结果": "审核通过",
            "【额外】塔塔二创": 1,
            "【额外】 是否推荐 ": true,
            "【额外】驳回": false,
            "【额外】备注": [{ "text": "cxxxx", "type": "text" }],
            "【额外】key": {
                "type": 1,
                "value": [
                    { "text": "integration-confidential-", "type": "text" },
                    { "text": "value", "type": "text" }
                ]
            },
            "【额外】权重": { "type": 2, "value": 1.5 },
            "【额外】启用": { "type": 7, "value": true },
            "【额外】标签组": ["A", "B"],
            "普通字段": "不会进入"
        }))];

        let updates = collect_reviewed_audit_results(
            &records,
            XingtuContentType::Video,
            "视频/图文ID",
            "审核结果",
        )
        .unwrap();

        assert_eq!(
            updates[0].audit_extra,
            json!({
                "塔塔二创": 1,
                "是否推荐": true,
                "驳回": false,
                "备注": "cxxxx",
                "key": "integration-confidential-value",
                "权重": 1.5,
                "启用": true,
                "标签组": ["A", "B"]
            })
        );
    }

    #[test]
    fn only_numeric_type_value_objects_are_treated_as_feishu_wrappers() {
        let business_object = json!({
            "type": "business",
            "value": [{ "text": "不能被静默扁平化", "type": "text" }]
        });
        assert_eq!(
            normalize_audit_extra_value(&business_object),
            business_object
        );

        let malformed_wrapper = json!({
            "type": 1,
            "value": [
                { "text": "A", "type": "text" },
                { "code": "B" }
            ]
        });
        assert_eq!(
            normalize_audit_extra_value(&malformed_wrapper),
            json!([
                { "text": "A", "type": "text" },
                { "code": "B" }
            ])
        );
    }

    #[test]
    fn missing_extra_fields_clear_audit_extra_to_an_empty_object() {
        let fields = json!({ "审核结果": "通过" });
        assert_eq!(
            collect_audit_extra(fields.as_object().unwrap()).unwrap(),
            json!({})
        );
    }

    #[test]
    fn rejects_conflicting_extra_fields_for_same_unique_key() {
        use crate::xingtu::activity_config::XingtuContentType;

        let records = vec![
            new_record(json!({
                "视频/图文ID": "video-1",
                "审核结果": "通过",
                "【额外】塔塔二创": true
            })),
            new_record(json!({
                "视频/图文ID": "video-1",
                "审核结果": "通过",
                "【额外】塔塔二创": false
            })),
        ];

        assert!(
            collect_reviewed_audit_results(
                &records,
                XingtuContentType::Video,
                "视频/图文ID",
                "审核结果"
            )
            .is_err()
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
