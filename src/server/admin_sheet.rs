use crate::lark::sheets::rich_text::{
    StyledCellWrite, collect_styled_cell_writes, parse_spreadsheet_location,
};
use crate::server::api::RequiredJsonBody;
use crate::server::error::{ApiError, ApiResult};
use crate::server::state::state_from_depot;
use salvo::http::header::{CACHE_CONTROL, HeaderValue, PRAGMA};
use salvo::oapi::ToSchema;
use salvo::prelude::*;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

const WRITE_BATCH_SIZE: usize = 10;

pub fn routes() -> Router {
    Router::with_path("feishu/spreadsheets/format-analysis").post(format_analysis_spreadsheet)
}

#[derive(Debug, Deserialize, Serialize, ToSchema)]
struct FormatAnalysisSpreadsheetRequest {
    /// 飞书普通电子表格链接，支持 /sheets/ 和带 sheet 参数的 /wiki/ 链接。
    url: String,
    /// 仅分析而不回写；默认 false。
    #[serde(default)]
    dry_run: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct FormattingSummary {
    sheet_id: String,
    matched_cells: usize,
    styled_segments: usize,
    updated_cells: usize,
    dry_run: bool,
    write_responses: Vec<Value>,
}

#[endpoint(
    tags("admin"),
    summary = "格式化电子表格分析文本",
    description = "从飞书普通电子表格链接提取 spreadsheetToken 和 sheet_id，以 sheet_id 读取单个范围；将“数字）文字：”加粗，将下降百分比标绿、上升百分比标红，绝对数字大于 50 时加粗。只回写命中的字符串单元格。"
)]
async fn format_analysis_spreadsheet(
    body: RequiredJsonBody<FormatAnalysisSpreadsheetRequest>,
    depot: &mut Depot,
    res: &mut Response,
) -> ApiResult<Value> {
    prevent_response_caching(res);
    let body = body.into_inner();
    let location = parse_spreadsheet_location(&body.url)
        .map_err(|error| ApiError::bad_request(error.to_string()))?;
    let state = state_from_depot(depot)?;

    let read = state
        .workflow
        .lark
        .get_openapi_json(
            &[
                "sheets",
                "v2",
                "spreadsheets",
                &location.spreadsheet_token,
                "values",
                &location.sheet_id,
            ],
            &[],
        )
        .await
        .map_err(ApiError::bad_gateway)?;
    if !is_successful_upstream(read.status, &read.body) {
        set_upstream_status(res, read.status);
        return Ok(Json(read.body));
    }

    let writes = collect_styled_cell_writes(&read.body, &location.sheet_id)
        .map_err(ApiError::bad_gateway)?;
    let styled_segments = writes.iter().map(|write| write.styled_segments).sum();
    let mut write_responses = Vec::new();

    if !body.dry_run {
        let total_batches = writes.len().div_ceil(WRITE_BATCH_SIZE);
        for (batch_index, batch) in writes.chunks(WRITE_BATCH_SIZE).enumerate() {
            let payload = build_batch_write_payload(batch);
            let write = state
                .workflow
                .lark
                .post_openapi_json(
                    &[
                        "sheets",
                        "v2",
                        "spreadsheets",
                        &location.spreadsheet_token,
                        "values_batch_update",
                    ],
                    &[],
                    &payload,
                )
                .await
                .map_err(ApiError::bad_gateway)?;
            if !is_successful_upstream(write.status, &write.body) {
                set_upstream_status(res, write.status);
                return Ok(Json(attach_failure_context(
                    write.body,
                    &location.spreadsheet_token,
                    &location.sheet_id,
                    batch_index,
                    total_batches,
                    writes.len(),
                )));
            }
            write_responses.push(write.body);
        }
    }

    let updated_cells = if body.dry_run { 0 } else { writes.len() };
    let summary = FormattingSummary {
        sheet_id: location.sheet_id,
        matched_cells: writes.len(),
        styled_segments,
        updated_cells,
        dry_run: body.dry_run,
        write_responses,
    };
    let mut response = read.body;
    attach_success_summary(&mut response, summary).map_err(ApiError::bad_gateway)?;
    Ok(Json(response))
}

fn build_batch_write_payload(writes: &[StyledCellWrite]) -> Value {
    let value_ranges = writes
        .iter()
        .map(|write| {
            json!({
                "range": normalize_write_range(&write.range),
                "values": [[write.value.clone()]]
            })
        })
        .collect::<Vec<_>>();
    json!({ "valueRanges": value_ranges })
}

/// 飞书批量写入接口要求单个单元格也使用闭区间（例如 `D1:D1`）。
/// 已经包含范围分隔符的多单元格、整列等合法范围保持不变。
fn normalize_write_range(range: &str) -> String {
    let Some((_, cell)) = range.rsplit_once('!') else {
        return range.to_owned();
    };
    if cell.contains(':') || !is_a1_cell(cell) {
        return range.to_owned();
    }

    format!("{range}:{cell}")
}

fn is_a1_cell(value: &str) -> bool {
    let value = value.strip_prefix('$').unwrap_or(value);
    let column_len = value.bytes().take_while(u8::is_ascii_alphabetic).count();
    if column_len == 0 || column_len == value.len() {
        return false;
    }

    let (column, row) = value.split_at(column_len);
    let row = row.strip_prefix('$').unwrap_or(row);
    column.bytes().all(|byte| byte.is_ascii_alphabetic())
        && !row.starts_with('0')
        && row.bytes().all(|byte| byte.is_ascii_digit())
        && !row.is_empty()
}

fn is_successful_upstream(status: u16, body: &Value) -> bool {
    (200..300).contains(&status) && body.get("code").and_then(Value::as_i64) == Some(0)
}

fn attach_success_summary(body: &mut Value, summary: FormattingSummary) -> anyhow::Result<()> {
    let data = body
        .get_mut("data")
        .and_then(Value::as_object_mut)
        .ok_or_else(|| anyhow::anyhow!("飞书读取响应缺少 data 对象"))?;
    data.insert("formatting".to_owned(), serde_json::to_value(summary)?);
    Ok(())
}

fn attach_failure_context(
    mut body: Value,
    spreadsheet_token: &str,
    sheet_id: &str,
    completed_batches: usize,
    total_batches: usize,
    matched_cells: usize,
) -> Value {
    if let Some(object) = body.as_object_mut() {
        object.insert(
            "operationContext".to_owned(),
            json!({
                "spreadsheetToken": spreadsheet_token,
                "sheetId": sheet_id,
                "completedBatches": completed_batches,
                "totalBatches": total_batches,
                "matchedCells": matched_cells
            }),
        );
    }
    body
}

fn prevent_response_caching(res: &mut Response) {
    res.headers_mut()
        .insert(CACHE_CONTROL, HeaderValue::from_static("no-store, private"));
    res.headers_mut()
        .insert(PRAGMA, HeaderValue::from_static("no-cache"));
}

fn set_upstream_status(res: &mut Response, status: u16) {
    res.status_code(StatusCode::from_u16(status).unwrap_or(StatusCode::BAD_GATEWAY));
}

#[cfg(test)]
mod tests {
    use super::*;
    use salvo::test::TestClient;

    #[test]
    fn batch_payload_uses_official_rich_text_cell_shape() {
        let writes = vec![StyledCellWrite {
            range: "HQ66KP!D2".to_owned(),
            value: json!([
                { "text": "播放", "type": "text" },
                {
                    "text": "↑69%",
                    "type": "text",
                    "segmentStyle": { "bold": true, "foreColor": "#D30000" }
                }
            ]),
            styled_segments: 1,
        }];
        let payload = build_batch_write_payload(&writes);
        assert_eq!(payload["valueRanges"][0]["range"], "HQ66KP!D2:D2");
        assert_eq!(
            payload["valueRanges"][0]["values"][0][0][1]["segmentStyle"]["foreColor"],
            "#D30000"
        );
    }

    #[test]
    fn single_cell_write_ranges_are_expanded_to_closed_ranges() {
        assert_eq!(normalize_write_range("HQ66KP!D1"), "HQ66KP!D1:D1");
        assert_eq!(normalize_write_range("HQ66KP!AA123"), "HQ66KP!AA123:AA123");
        assert_eq!(normalize_write_range("HQ66KP!$D$1"), "HQ66KP!$D$1:$D$1");
    }

    #[test]
    fn existing_multi_cell_and_column_ranges_are_preserved() {
        for range in ["HQ66KP!A1:B5", "HQ66KP!A:B", "HQ66KP!A2:B", "HQ66KP"] {
            assert_eq!(normalize_write_range(range), range);
        }
    }

    #[test]
    fn malformed_single_addresses_are_not_rewritten() {
        for range in [
            "HQ66KP!1D",
            "HQ66KP!D",
            "HQ66KP!D0",
            "HQ66KP!D-1",
            "HQ66KP!D$$1",
            "HQ66KP!$$D1",
        ] {
            assert_eq!(normalize_write_range(range), range);
        }
    }

    #[test]
    fn upstream_requires_http_success_and_feishu_code_zero() {
        assert!(is_successful_upstream(200, &json!({ "code": 0 })));
        assert!(!is_successful_upstream(500, &json!({ "code": 0 })));
        assert!(!is_successful_upstream(200, &json!({ "code": 1254040 })));
    }

    #[tokio::test]
    async fn spreadsheet_content_response_is_never_cacheable() {
        #[handler]
        async fn probe(res: &mut Response) {
            prevent_response_caching(res);
        }

        let response = TestClient::get("http://127.0.0.1/")
            .send(&Service::new(Router::new().get(probe)))
            .await;
        assert_eq!(
            response.headers().get(CACHE_CONTROL).unwrap(),
            "no-store, private"
        );
        assert_eq!(response.headers().get(PRAGMA).unwrap(), "no-cache");
    }
}
