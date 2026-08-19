use regex::Regex;
use serde_json::{Value, json};
use std::sync::LazyLock;
use url::Url;

use super::column_number_to_name;

static ANALYSIS_TOKEN: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\d+）[^：\r\n]+：|[↓↑]\d+(?:\.\d+)?%")
        .expect("analysis rich-text regex must be valid")
});

#[derive(Debug, Clone, PartialEq)]
pub struct SpreadsheetLocation {
    pub spreadsheet_token: String,
    pub sheet_id: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct StyledCellWrite {
    pub range: String,
    pub value: Value,
    pub styled_segments: usize,
}

/// 解析飞书电子表格链接。兼容直接 URL 和 Markdown 链接，并支持 `/sheets/` 与
/// 用户实际使用的 `/wiki/` 路径。
pub fn parse_spreadsheet_location(input: &str) -> anyhow::Result<SpreadsheetLocation> {
    let value = extract_link_target(input);
    let url = Url::parse(value).map_err(|_| anyhow::anyhow!("url 必须是合法的飞书 HTTPS 链接"))?;
    anyhow::ensure!(url.scheme() == "https", "url 必须使用 HTTPS");

    let host = url.host_str().unwrap_or_default().to_ascii_lowercase();
    anyhow::ensure!(
        host == "feishu.cn"
            || host.ends_with(".feishu.cn")
            || host == "larksuite.com"
            || host.ends_with(".larksuite.com"),
        "url 域名必须属于 feishu.cn 或 larksuite.com"
    );

    let segments = url
        .path_segments()
        .map(|segments| {
            segments
                .filter(|value| !value.is_empty())
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    anyhow::ensure!(
        segments.len() == 2 && matches!(segments[0], "sheets" | "wiki"),
        "url 路径必须是 /sheets/{{spreadsheet_token}} 或 /wiki/{{spreadsheet_token}}"
    );

    let spreadsheet_token = validate_token(segments[1], "spreadsheet_token")?;
    let sheet_id = url
        .query_pairs()
        .find_map(|(key, value)| (key == "sheet").then(|| value.into_owned()))
        .ok_or_else(|| anyhow::anyhow!("url 缺少 sheet 查询参数"))?;
    let sheet_id = validate_token(&sheet_id, "sheet_id")?;

    Ok(SpreadsheetLocation {
        spreadsheet_token,
        sheet_id,
    })
}

/// 从飞书读取响应中找出需要回写的字符串单元格，并生成局部富文本值。
pub fn collect_styled_cell_writes(
    body: &Value,
    sheet_id: &str,
) -> anyhow::Result<Vec<StyledCellWrite>> {
    let value_range = body
        .pointer("/data/valueRange")
        .and_then(Value::as_object)
        .ok_or_else(|| anyhow::anyhow!("飞书读取响应缺少 data.valueRange"))?;
    let effective_range = value_range
        .get("range")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow::anyhow!("飞书读取响应缺少 data.valueRange.range"))?;
    let (start_column, start_row) = parse_a1_start(effective_range)?;
    let Some(values) = value_range.get("values").and_then(Value::as_array) else {
        return Ok(Vec::new());
    };

    let mut writes = Vec::new();
    for (row_offset, row) in values.iter().enumerate() {
        let Some(row) = row.as_array() else {
            continue;
        };
        for (column_offset, cell) in row.iter().enumerate() {
            let Some(text) = cell.as_str() else {
                continue;
            };
            let Some((segments, styled_segments)) = style_analysis_text(text) else {
                continue;
            };
            let column_number = start_column + column_offset;
            let row_number = start_row + row_offset;
            writes.push(StyledCellWrite {
                range: format!(
                    "{}!{}{}",
                    sheet_id,
                    column_number_to_name(column_number as i32),
                    row_number
                ),
                value: Value::Array(segments),
                styled_segments,
            });
        }
    }
    Ok(writes)
}

/// 将一段文本拆成飞书 Sheets 富文本片段。没有规则命中时返回 `None`，调用方无需回写。
pub fn style_analysis_text(text: &str) -> Option<(Vec<Value>, usize)> {
    let matches = ANALYSIS_TOKEN.find_iter(text).collect::<Vec<_>>();
    if matches.is_empty() {
        return None;
    }

    let mut segments = Vec::with_capacity(matches.len() * 2 + 1);
    let mut cursor = 0;
    for matched in &matches {
        if matched.start() > cursor {
            segments.push(text_segment(&text[cursor..matched.start()], None));
        }

        let matched_text = matched.as_str();
        let style = if matched_text.starts_with(['↓', '↑']) {
            let percentage = matched_text[3..matched_text.len() - 1]
                .parse::<f64>()
                .unwrap_or_default();
            let mut style = serde_json::Map::new();
            style.insert(
                "foreColor".to_owned(),
                Value::String(
                    if matched_text.starts_with('↓') {
                        "#00D100"
                    } else {
                        "#D30000"
                    }
                    .to_owned(),
                ),
            );
            if percentage > 50.0 {
                style.insert("bold".to_owned(), Value::Bool(true));
            }
            Value::Object(style)
        } else {
            json!({ "bold": true })
        };
        segments.push(text_segment(matched_text, Some(style)));
        cursor = matched.end();
    }
    if cursor < text.len() {
        segments.push(text_segment(&text[cursor..], None));
    }

    Some((segments, matches.len()))
}

fn text_segment(text: &str, style: Option<Value>) -> Value {
    let mut segment = serde_json::Map::new();
    segment.insert("text".to_owned(), Value::String(text.to_owned()));
    segment.insert("type".to_owned(), Value::String("text".to_owned()));
    if let Some(style) = style {
        segment.insert("segmentStyle".to_owned(), style);
    }
    Value::Object(segment)
}

fn extract_link_target(input: &str) -> &str {
    let input = input.trim().trim_matches(['"', '\'']);
    if input.starts_with('[') {
        input
            .rsplit_once("](")
            .and_then(|(_, target)| target.strip_suffix(')'))
            .unwrap_or(input)
            .trim()
    } else {
        input
    }
}

fn validate_token(value: &str, field: &str) -> anyhow::Result<String> {
    let value = value.trim();
    anyhow::ensure!(
        !value.is_empty()
            && value.len() <= 128
            && value
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-')),
        "链接中的 {field} 格式不合法"
    );
    Ok(value.to_owned())
}

fn parse_a1_start(range: &str) -> anyhow::Result<(usize, usize)> {
    let start = range
        .rsplit_once('!')
        .map(|(_, cells)| cells)
        .unwrap_or(range)
        .split(':')
        .next()
        .unwrap_or_default()
        .replace('$', "");
    let letters_len = start.bytes().take_while(u8::is_ascii_alphabetic).count();
    anyhow::ensure!(
        letters_len > 0 && letters_len < start.len(),
        "飞书返回了无效的 A1 范围"
    );
    let (letters, row) = start.split_at(letters_len);
    let row = row
        .parse::<usize>()
        .map_err(|_| anyhow::anyhow!("飞书返回了无效的 A1 行号"))?;
    anyhow::ensure!(row > 0, "飞书返回了无效的 A1 行号");

    let mut column = 0_usize;
    for byte in letters.bytes() {
        column = column
            .checked_mul(26)
            .and_then(|value| value.checked_add((byte.to_ascii_uppercase() - b'A' + 1) as usize))
            .ok_or_else(|| anyhow::anyhow!("飞书返回的 A1 列号过大"))?;
    }
    Ok((column, row))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_user_wiki_markdown_link() {
        let location = parse_spreadsheet_location(
            "[https://example.feishu.cn/wiki/ExampleSpreadsheetToken?sheet=Sheet01](https://example.feishu.cn/wiki/ExampleSpreadsheetToken?sheet=Sheet01)",
        )
        .unwrap();
        assert_eq!(location.spreadsheet_token, "ExampleSpreadsheetToken");
        assert_eq!(location.sheet_id, "Sheet01");
    }

    #[test]
    fn rejects_untrusted_or_incomplete_links() {
        assert!(parse_spreadsheet_location("https://example.com/sheets/token?sheet=id").is_err());
        assert!(parse_spreadsheet_location("https://example.feishu.cn/sheets/token").is_err());
    }

    #[test]
    fn applies_heading_and_percentage_styles() {
        let (segments, count) =
            style_analysis_text("1）实况解说类：人数↓29%，播放↑69%，边界↑50%，小数↓50.1%").unwrap();
        assert_eq!(count, 5);
        assert_eq!(segments[0]["segmentStyle"]["bold"], true);
        assert_eq!(segments[2]["segmentStyle"]["foreColor"], "#00D100");
        assert!(segments[2]["segmentStyle"].get("bold").is_none());
        assert_eq!(segments[4]["segmentStyle"]["foreColor"], "#D30000");
        assert_eq!(segments[4]["segmentStyle"]["bold"], true);
        assert!(segments[6]["segmentStyle"].get("bold").is_none());
        assert_eq!(segments[8]["segmentStyle"]["bold"], true);
    }

    #[test]
    fn creates_cell_ranges_from_effective_read_range_and_skips_non_strings() {
        let body = json!({
            "data": {
                "valueRange": {
                    "range": "Sheet01!C4:D5",
                    "values": [
                        [true, "1）攻略类：人数↑51%"],
                        [42, "普通文字"]
                    ]
                }
            }
        });
        let writes = collect_styled_cell_writes(&body, "Sheet01").unwrap();
        assert_eq!(writes.len(), 1);
        assert_eq!(writes[0].range, "Sheet01!D4");
        assert_eq!(writes[0].styled_segments, 2);
        assert!(writes[0].value.is_array());
    }
}
