use crate::client::LarkClient;
use anyhow::{Context, anyhow};
use open_lark::docs::base::bitable::{CreateRecordItem, Record as BitableRecord, UpdateRecordItem};
use open_lark::docs::common::api_endpoints::BitableApiV1;
use open_lark::prelude::{ApiRequest, Transport};
use serde_json::{Value, json};
use std::collections::BTreeSet;
use tokio::time::{Duration, sleep};

/// 飞书批量接口单批数量。
///
/// 先沿用 006 里跑通的 1000。后续如果接口限制变化，只需要改这里。
const BATCH_SIZE: usize = 1000;

/// 带重试读取多维表格记录。
///
/// 飞书接口偶发失败时直接终止整条同步会比较可惜，所以读取侧保留简单退避重试。
pub async fn read_existing_records_with_retry(
    lark: &LarkClient,
    app_token: &str,
    table_id: &str,
) -> anyhow::Result<Vec<BitableRecord>> {
    let max_retries = 3;

    for attempt in 1..=max_retries {
        let result = lark
            .raw()
            .docs
            .search_bitable_records_all(app_token, table_id)
            .await;

        match result {
            Ok(records) => return Ok(records),
            Err(err) => {
                tracing::error!("第 {} 次读取多维表格记录失败：{}", attempt, err);

                if attempt == max_retries {
                    return Err(err).context("多次读取多维表格记录失败");
                }

                sleep(Duration::from_secs(attempt as u64 * 2)).await;
            }
        }
    }

    Err(anyhow!("读取多维表格记录失败"))
}

/// 带重试读取可选的多维表格记录。
///
/// 主要用于手动登记表：当表为空时，飞书/SDK 可能返回 `data: null`，
/// 这类情况对手动表应视为“没有手动登记数据”，而不是让整条同步失败。
pub async fn read_optional_records_with_retry(
    lark: &LarkClient,
    app_token: &str,
    table_id: &str,
) -> anyhow::Result<Vec<BitableRecord>> {
    match read_existing_records_with_retry(lark, app_token, table_id).await {
        Ok(records) => Ok(records),
        Err(err) if is_missing_response_data_error(&err) => {
            tracing::error!(
                "读取可选多维表格 `{}` 返回空 data，按 0 条记录继续：{}",
                table_id,
                err
            );
            Ok(Vec::new())
        }
        Err(err) => Err(err),
    }
}

/// 判断是否是 SDK 对空 `response.data` 的校验错误。
fn is_missing_response_data_error(err: &anyhow::Error) -> bool {
    err.chain().any(|cause| {
        let message = cause.to_string();

        message.contains("response.data") || message.contains("服务器没有返回有效的数据")
    })
}

/// 执行多维表格写入请求。
///
/// openlark 当前的 batch create/update 封装会直接 unwrap `response.data`。
/// 但飞书有些成功写入响应可能没有 SDK 期望的数据结构，所以这里改为自己检查原始 code：
/// - code = 0：飞书已接受写入，按成功处理
/// - code != 0：保留飞书原始 code/msg，方便定位字段、权限等真实错误
async fn execute_bitable_write_request(
    lark: &LarkClient,
    endpoint: BitableApiV1,
    body: Value,
    operation: &str,
) -> anyhow::Result<()> {
    let request = ApiRequest::<Value>::post(endpoint.to_url()).body(body);
    let response = Transport::<Value>::request(request, lark.raw().config(), None).await?;

    if response.is_success() {
        return Ok(());
    }

    Err(anyhow!(
        "{}失败，飞书返回 code={} msg={} request_id={}",
        operation,
        response.code(),
        response.message(),
        response
            .raw_response
            .request_id
            .as_deref()
            .unwrap_or("<none>")
    ))
}

/// 批量更新多维表格记录。
pub async fn batch_update_records(
    lark: &LarkClient,
    app_token: &str,
    table_id: &str,
    records: Vec<UpdateRecordItem>,
) -> anyhow::Result<()> {
    if records.is_empty() {
        tracing::info!("没有需要更新的记录");
        return Ok(());
    }

    let write_fields = collect_record_field_names(records.iter().map(|record| &record.fields));
    validate_bitable_write_fields(lark, app_token, table_id, &write_fields, "批量更新")
        .await
        .context("校验批量更新字段失败")?;

    let total = records.len();

    for (index, chunk) in records.chunks(BATCH_SIZE).enumerate() {
        let chunk_fields = collect_record_field_names(chunk.iter().map(|record| &record.fields));
        execute_bitable_write_request(
            lark,
            BitableApiV1::RecordBatchUpdate(app_token.to_string(), table_id.to_string()),
            json!({ "records": chunk }),
            "批量更新记录",
        )
        .await
        .with_context(|| {
            format!(
                "第 {} 批更新失败；本批写入字段=[{}]",
                index + 1,
                format_field_names(&chunk_fields)
            )
        })?;

        let finished = ((index + 1) * BATCH_SIZE).min(total);

        tracing::info!(
            "第 {} 批更新完成，本批 {} 条，累计 {}/{}",
            index + 1,
            chunk.len(),
            finished,
            total
        );
    }

    Ok(())
}

#[cfg(test)]
#[allow(clippy::items_after_test_module)]
mod tests {
    use super::*;
    use anyhow::Context;

    #[test]
    fn missing_response_data_error_matches_context_chain() {
        let err = Err::<(), _>(anyhow!("验证错误 response.data: 服务器没有返回有效的数据"))
            .context("多次读取多维表格记录失败")
            .unwrap_err();

        assert!(is_missing_response_data_error(&err));
    }

    #[test]
    fn field_diagnostics_reports_exact_missing_names() {
        let records = [
            json!({ "稿件ID": "1", "播放量": 10 }),
            json!({ "稿件ID": "2", "已删除字段": 20 }),
        ];
        let write_fields = collect_record_field_names(records.iter());
        let table_fields = BTreeSet::from(["稿件ID".to_string(), "播放量".to_string()]);

        assert_eq!(
            missing_field_names(&write_fields, &table_fields),
            BTreeSet::from(["已删除字段".to_string()])
        );
    }

    #[test]
    fn field_page_parser_only_requires_field_name() {
        let page = parse_bitable_field_page(&json!({
            "has_more": false,
            "items": [
                {
                    "field_id": "fld1",
                    "field_name": "视频/图文ID",
                    "type": 1
                }
            ]
        }))
        .unwrap();

        assert_eq!(
            page.field_names,
            BTreeSet::from(["视频/图文ID".to_string()])
        );
        assert!(!page.has_more);
        assert_eq!(page.page_token, None);
    }
}

/// 批量创建多维表格记录。
pub async fn batch_create_records(
    lark: &LarkClient,
    app_token: &str,
    table_id: &str,
    records: Vec<CreateRecordItem>,
) -> anyhow::Result<()> {
    if records.is_empty() {
        tracing::info!("没有需要创建的记录");
        return Ok(());
    }

    let write_fields = collect_record_field_names(records.iter().map(|record| &record.fields));
    validate_bitable_write_fields(lark, app_token, table_id, &write_fields, "批量创建")
        .await
        .context("校验批量创建字段失败")?;

    let total = records.len();

    for (index, chunk) in records.chunks(BATCH_SIZE).enumerate() {
        let chunk_fields = collect_record_field_names(chunk.iter().map(|record| &record.fields));
        execute_bitable_write_request(
            lark,
            BitableApiV1::RecordBatchCreate(app_token.to_string(), table_id.to_string()),
            json!({ "records": chunk }),
            "批量创建记录",
        )
        .await
        .with_context(|| {
            format!(
                "第 {} 批创建失败；本批写入字段=[{}]",
                index + 1,
                format_field_names(&chunk_fields)
            )
        })?;

        let finished = ((index + 1) * BATCH_SIZE).min(total);

        tracing::info!(
            "第 {} 批创建完成，本批 {} 条，累计 {}/{}",
            index + 1,
            chunk.len(),
            finished,
            total
        );
    }

    Ok(())
}

async fn validate_bitable_write_fields(
    lark: &LarkClient,
    app_token: &str,
    table_id: &str,
    write_fields: &BTreeSet<String>,
    operation: &str,
) -> anyhow::Result<()> {
    if write_fields.is_empty() {
        return Ok(());
    }

    let table_fields = match read_bitable_field_names(lark, app_token, table_id).await {
        Ok(field_names) => field_names,
        Err(err) => {
            tracing::warn!(
                table_id,
                operation,
                write_fields = %format_field_names(write_fields),
                error = ?err,
                "读取目标多维表字段失败，跳过字段预检并继续写入"
            );
            return Ok(());
        }
    };
    let missing_fields = missing_field_names(write_fields, &table_fields);

    if missing_fields.is_empty() {
        tracing::info!(
            table_id,
            operation,
            write_field_count = write_fields.len(),
            "目标多维表字段校验通过"
        );
        return Ok(());
    }

    Err(anyhow!(
        "目标多维表缺少写入字段：table_id={} operation={} missing_fields=[{}] write_fields=[{}] table_fields=[{}]",
        table_id,
        operation,
        format_field_names(&missing_fields),
        format_field_names(write_fields),
        format_field_names(&table_fields)
    ))
}

pub async fn read_bitable_field_names(
    lark: &LarkClient,
    app_token: &str,
    table_id: &str,
) -> anyhow::Result<BTreeSet<String>> {
    let mut field_names = BTreeSet::new();
    let mut page_token = None::<String>;

    loop {
        let endpoint = BitableApiV1::FieldList(app_token.to_string(), table_id.to_string());
        let mut request = ApiRequest::<Value>::get(endpoint.to_url()).query("page_size", "100");
        if let Some(token) = page_token.take() {
            request = request.query("page_token", token);
        }

        let response = Transport::<Value>::request(request, lark.raw().config(), None)
            .await
            .context("调用飞书字段列表接口失败")?;
        if !response.is_success() {
            return Err(anyhow!(
                "飞书字段列表接口失败：code={} msg={} request_id={}",
                response.code(),
                response.message(),
                response
                    .raw_response
                    .request_id
                    .as_deref()
                    .unwrap_or("<none>")
            ));
        }
        let data = response
            .data
            .ok_or_else(|| anyhow!("飞书字段列表接口成功但没有返回 data"))?;
        let page = parse_bitable_field_page(&data)?;
        field_names.extend(page.field_names);

        if !page.has_more {
            break;
        }
        page_token = Some(
            page.page_token
                .filter(|token| !token.trim().is_empty())
                .ok_or_else(|| anyhow!("飞书字段列表仍有下一页，但未返回 page_token"))?,
        );
    }

    Ok(field_names)
}

struct BitableFieldPage {
    field_names: BTreeSet<String>,
    has_more: bool,
    page_token: Option<String>,
}

fn parse_bitable_field_page(data: &Value) -> anyhow::Result<BitableFieldPage> {
    let items = data
        .get("items")
        .and_then(Value::as_array)
        .ok_or_else(|| anyhow!("飞书字段列表 data.items 缺失或不是数组"))?;
    let mut field_names = BTreeSet::new();

    for (index, item) in items.iter().enumerate() {
        let field_name = item
            .get("field_name")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| anyhow!("飞书字段列表 items[{index}].field_name 缺失或为空"))?;
        field_names.insert(field_name.to_string());
    }

    Ok(BitableFieldPage {
        field_names,
        has_more: data
            .get("has_more")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        page_token: data
            .get("page_token")
            .and_then(Value::as_str)
            .map(str::to_string),
    })
}

fn collect_record_field_names<'a>(
    field_values: impl IntoIterator<Item = &'a Value>,
) -> BTreeSet<String> {
    field_values
        .into_iter()
        .filter_map(Value::as_object)
        .flat_map(|fields| fields.keys().cloned())
        .collect()
}

fn missing_field_names(
    write_fields: &BTreeSet<String>,
    table_fields: &BTreeSet<String>,
) -> BTreeSet<String> {
    write_fields.difference(table_fields).cloned().collect()
}

fn format_field_names(field_names: &BTreeSet<String>) -> String {
    field_names.iter().cloned().collect::<Vec<_>>().join(", ")
}
