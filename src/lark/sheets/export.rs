use serde_json::{Map, Value};
use std::fs::File;
use std::path::Path;

/// 将飞书表格二维 values 转成 JSON records。
///
/// 输入格式示例：
/// [
///   ["视频/图文ID", "发布时间", "作者名称"],
///   ["demo-video-id", "2026-06-01 00:28:25", "示例作者"]
/// ]
///
/// 输出格式示例：
/// [
///   {
///     "视频/图文ID": "demo-video-id",
///     "发布时间": "2026-06-01 00:28:25",
///     "作者名称": "示例作者"
///   }
/// ]
pub fn values_to_json_records(values: &[Vec<Value>]) -> anyhow::Result<Vec<Map<String, Value>>> {
    if values.is_empty() {
        anyhow::bail!("sheet values 为空，无法导出");
    }

    let header_row = &values[0];

    let headers: Vec<String> = header_row.iter().map(value_to_plain_string).collect();

    if headers.is_empty() {
        anyhow::bail!("表头为空，无法导出");
    }

    let mut records = Vec::new();

    for row in values.iter().skip(1) {
        // 跳过完全空行
        if row.iter().all(is_empty_value) {
            continue;
        }

        let mut record = Map::new();

        for (index, header) in headers.iter().enumerate() {
            if header.trim().is_empty() {
                continue;
            }

            let cell_value = row.get(index).cloned().unwrap_or(Value::Null);

            // JSON 导出也做换行清理，保证 JSON 和 CSV 内容一致
            let cleaned_value = clean_json_value(cell_value);

            record.insert(header.clone(), cleaned_value);
        }

        records.push(record);
    }

    Ok(records)
}

/// 同时保存 JSON 和 CSV。
///
/// json_path:
/// - 例如 "sheet_data.json"
///
/// csv_path:
/// - 例如 "sheet_data.csv"
pub fn export_values_to_json_and_csv(
    values: &[Vec<Value>],
    json_path: &str,
    csv_path: &str,
) -> anyhow::Result<()> {
    if values.is_empty() {
        anyhow::bail!("sheet values 为空，无法导出");
    }

    let headers: Vec<String> = values[0]
        .iter()
        .map(value_to_plain_string)
        .filter(|header| !header.trim().is_empty())
        .collect();

    if headers.is_empty() {
        anyhow::bail!("表头为空，无法导出");
    }

    let records = values_to_json_records(values)?;

    save_json(&records, json_path)?;
    save_csv(&records, &headers, csv_path)?;

    println!("JSON 已保存到: {}", json_path);
    println!("CSV 已保存到: {}", csv_path);
    println!("导出数据行数: {}", records.len());

    Ok(())
}

/// 保存 JSON 文件。
pub fn save_json<P: AsRef<Path>>(records: &[Map<String, Value>], path: P) -> anyhow::Result<()> {
    let file = File::create(path)?;
    serde_json::to_writer_pretty(file, records)?;
    Ok(())
}

/// 保存 CSV 文件。
///
/// 注意：
/// CSV 只能保存二维纯文本。
///
/// 如果单元格里是 Array/Object，例如飞书链接富文本，
/// 会被转成 JSON 字符串后再清理换行。
pub fn save_csv<P: AsRef<Path>>(
    records: &[Map<String, Value>],
    headers: &[String],
    path: P,
) -> anyhow::Result<()> {
    let mut writer = csv::Writer::from_path(path)?;

    writer.write_record(headers)?;

    for record in records {
        let row: Vec<String> = headers
            .iter()
            .map(|header| {
                record
                    .get(header)
                    .map(value_to_plain_string)
                    .unwrap_or_default()
            })
            .collect();

        writer.write_record(row)?;
    }

    writer.flush()?;

    Ok(())
}

/// 将 serde_json::Value 转成适合 CSV 的纯文本。
///
/// 规则：
/// - String -> 原字符串，并清理换行
/// - Number -> 数字字符串
/// - Bool -> true / false
/// - Null -> 空字符串
/// - Array/Object -> JSON 字符串，并清理换行
pub fn value_to_plain_string(value: &Value) -> String {
    let raw = match value {
        Value::String(s) => s.clone(),
        Value::Number(n) => n.to_string(),
        Value::Bool(b) => b.to_string(),
        Value::Null => String::new(),
        Value::Array(_) | Value::Object(_) => serde_json::to_string(value).unwrap_or_default(),
    };

    clean_cell_text(&raw)
}

/// 清理单元格文本。
///
/// 用于导出 CSV / JSON：
/// - 将 \r\n 替换为空格
/// - 将 \n 替换为空格
/// - 将 \r 替换为空格
/// - 将制表符、多余空格压缩为单个空格
/// - 去除首尾空白
///
/// 这样可以避免 CSV 中出现单元格内换行，影响后续处理。
pub fn clean_cell_text(input: &str) -> String {
    input
        .replace("\r\n", " ")
        .replace(['\n', '\r'], " ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// 清理 JSON Value 中的字符串换行。
///
/// 规则：
/// - String：清理换行
/// - Array：递归清理数组内元素
/// - Object：递归清理对象内字段
/// - Number / Bool / Null：保持原样
pub fn clean_json_value(value: Value) -> Value {
    match value {
        Value::String(s) => Value::String(clean_cell_text(&s)),

        Value::Array(arr) => {
            let cleaned_arr = arr.into_iter().map(clean_json_value).collect();

            Value::Array(cleaned_arr)
        }

        Value::Object(obj) => {
            let cleaned_obj = obj
                .into_iter()
                .map(|(key, value)| (key, clean_json_value(value)))
                .collect();

            Value::Object(cleaned_obj)
        }

        Value::Number(_) | Value::Bool(_) | Value::Null => value,
    }
}

/// 判断一个单元格是否为空。
fn is_empty_value(value: &Value) -> bool {
    match value {
        Value::Null => true,
        Value::String(s) => s.trim().is_empty(),
        Value::Array(arr) => arr.is_empty(),
        Value::Object(obj) => obj.is_empty(),
        Value::Number(_) | Value::Bool(_) => false,
    }
}
