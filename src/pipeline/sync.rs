use anyhow::{Context, anyhow};
use chrono::{NaiveDateTime, TimeZone};
use chrono_tz::Asia::Shanghai;
use open_lark::docs::base::bitable::{CreateRecordItem, Record as BitableRecord, UpdateRecordItem};
use serde_json::{Map, Number, Value, json};
use std::collections::{HashMap, HashSet};

/// Sheet 中解析出来的一行待同步数据。
///
/// `unique_key` 是业务唯一键，例如视频同步里的 `视频/图文ID`。
/// `fields` 是已经转换成多维表格可写入格式的字段集合。
#[derive(Debug, Clone)]
pub struct SourceRow {
    pub unique_key: String,
    pub fields: Map<String, Value>,
}

/// 已存在的多维表格记录索引项。
///
/// 这里保留原始字段，是为了后续做语义 diff，避免无变化字段重复更新。
#[derive(Debug, Clone)]
pub struct ExistingRecordIndexItem {
    pub record_id: String,
    pub fields: Map<String, Value>,
}

/// Upsert 计划。
///
/// create 和 update 分开存放，方便 pipeline 在不同 step 中分别执行。
#[derive(Debug, Default)]
pub struct UpsertPlan {
    pub need_create_records: Vec<CreateRecordItem>,
    pub need_update_records: Vec<UpdateRecordItem>,
}

/// 单个业务字段的值类型。
///
/// 不同同步任务可以用这组类型描述“Sheet 值应该怎么写入多维表格”。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FieldValueKind {
    /// 不做额外转换，只做基础空值清洗。
    Passthrough,
    /// 普通文本字段。
    Text,
    /// 数字字段。
    Number,
    /// 日期字段，写入毫秒时间戳。
    TimestampMillis,
    /// URL 字段，写入 `{ text, link }` 结构。
    Url,
}

/// 字段转换和过滤规则。
///
/// 这层抽象用来隔离“通用同步流程”和“具体业务字段差异”：
/// 视频、直播可以共用解析/diff/upsert，只替换字段规则。
#[derive(Debug, Clone)]
pub struct FieldValueRules {
    skip_fields: HashSet<String>,
    text_fields: HashSet<String>,
    number_fields: HashSet<String>,
    timestamp_millis_fields: HashSet<String>,
    url_fields: HashSet<String>,
}

impl FieldValueRules {
    /// 创建一组空规则。
    pub fn new() -> Self {
        Self {
            skip_fields: HashSet::new(),
            text_fields: HashSet::new(),
            number_fields: HashSet::new(),
            timestamp_millis_fields: HashSet::new(),
            url_fields: HashSet::new(),
        }
    }

    /// 声明不从 Sheet 同步到多维表格的字段。
    pub fn with_skip_fields(mut self, fields: &[&str]) -> Self {
        self.skip_fields.extend(normalize_field_names(fields));
        self
    }

    /// 声明文本字段。
    pub fn with_text_fields(mut self, fields: &[&str]) -> Self {
        self.text_fields.extend(normalize_field_names(fields));
        self
    }

    /// 声明数字字段。
    pub fn with_number_fields(mut self, fields: &[&str]) -> Self {
        self.number_fields.extend(normalize_field_names(fields));
        self
    }

    /// 声明时间戳毫秒字段。
    pub fn with_timestamp_millis_fields(mut self, fields: &[&str]) -> Self {
        self.timestamp_millis_fields
            .extend(normalize_field_names(fields));
        self
    }

    /// 声明 URL 字段。
    pub fn with_url_fields(mut self, fields: &[&str]) -> Self {
        self.url_fields.extend(normalize_field_names(fields));
        self
    }

    /// 判断字段是否应该跳过同步。
    pub fn should_skip_field(&self, field_name: &str) -> bool {
        self.skip_fields.contains(&normalize_field_name(field_name))
    }

    /// 获取字段写入类型。
    pub fn value_kind(&self, field_name: &str) -> FieldValueKind {
        let normalized = normalize_field_name(field_name);

        if self.url_fields.contains(&normalized) {
            FieldValueKind::Url
        } else if self.timestamp_millis_fields.contains(&normalized) {
            FieldValueKind::TimestampMillis
        } else if self.number_fields.contains(&normalized) {
            FieldValueKind::Number
        } else if self.text_fields.contains(&normalized) {
            FieldValueKind::Text
        } else {
            FieldValueKind::Passthrough
        }
    }

    /// 把 Sheet 单元格值转换成多维表格 API 可写入的值。
    pub fn to_bitable_write_value(&self, field_name: &str, value: Value) -> Value {
        let value = normalize_cell_value(value);

        if value.is_null() {
            return Value::Null;
        }

        match self.value_kind(field_name) {
            FieldValueKind::Passthrough => value,
            FieldValueKind::Text => value_to_plain_string(&value)
                .map(Value::String)
                .unwrap_or(Value::Null),
            FieldValueKind::Number => to_number_value(value),
            FieldValueKind::TimestampMillis => to_timestamp_millis_value(value),
            FieldValueKind::Url => {
                let Some(url) = value_to_plain_string(&value) else {
                    return Value::Null;
                };

                json!({
                    "text": url,
                    "link": url
                })
            }
        }
    }
}

impl Default for FieldValueRules {
    fn default() -> Self {
        Self::new()
    }
}

/// 收集源数据中的唯一键，并保持首次出现顺序。
///
/// audit 表同步只需要唯一键列表，所以这里提前去重。
pub fn collect_unique_keys(source_rows: &[SourceRow]) -> Vec<String> {
    let mut keys = Vec::new();
    let mut seen = HashSet::new();

    for row in source_rows {
        if seen.insert(row.unique_key.clone()) {
            keys.push(row.unique_key.clone());
        }
    }

    keys
}

/// Sheet rows -> SourceRow。
///
/// 默认第一行是表头。字段转换由 `rules` 决定，
/// 因此同一套解析流程可以服务视频、直播等不同业务。
pub fn parse_sheet_rows(
    rows: Vec<Vec<Value>>,
    unique_key_field: &str,
    rules: &FieldValueRules,
) -> anyhow::Result<Vec<SourceRow>> {
    let mut iter = rows.into_iter();

    let header_row = iter.next().ok_or_else(|| anyhow!("Sheet 为空，缺少表头"))?;
    let headers = parse_headers(header_row).context("解析表头失败")?;

    if !headers
        .iter()
        .any(|h| normalize_field_name(h) == normalize_field_name(unique_key_field))
    {
        return Err(anyhow!(
            "Sheet 表头中找不到唯一键字段：{}",
            unique_key_field
        ));
    }

    let mut result = Vec::new();
    let mut first_row_by_key = HashMap::new();

    for (row_index, row) in iter.enumerate() {
        let display_row_number = row_index + 2;

        if is_empty_row(&row) {
            continue;
        }

        let mut fields = Map::new();

        for (column_index, header) in headers.iter().enumerate() {
            if rules.should_skip_field(header) {
                continue;
            }

            let raw_value = row.get(column_index).cloned().unwrap_or(Value::Null);
            let write_value = rules.to_bitable_write_value(header, raw_value);

            if !write_value.is_null() {
                fields.insert(header.clone(), write_value);
            }
        }

        let unique_key = find_field_value_by_name(&fields, unique_key_field)
            .and_then(value_to_key_string)
            .ok_or_else(|| {
                anyhow!(
                    "第 {} 行缺少唯一键字段 `{}`",
                    display_row_number,
                    unique_key_field
                )
            })?;

        if let Some(first_row_number) =
            first_row_by_key.insert(unique_key.clone(), display_row_number)
        {
            return Err(anyhow!(
                "Sheet 唯一键字段 `{}` 存在重复值 `{}`：第 {} 行与第 {} 行",
                unique_key_field,
                unique_key,
                first_row_number,
                display_row_number
            ));
        }

        result.push(SourceRow { unique_key, fields });
    }

    Ok(result)
}

/// 多维表格记录 -> SourceRow。
///
/// 手动登记数据来自多维表格，而不是 Sheet 二维数组。这里会把读取到的飞书字段结构
/// 重新转换成主表可写入的格式，并跳过没有唯一键的草稿/空记录。
pub fn parse_bitable_records_as_source_rows(
    records: &[BitableRecord],
    unique_key_field: &str,
    rules: &FieldValueRules,
) -> anyhow::Result<Vec<SourceRow>> {
    let mut result = Vec::new();
    let mut first_record_id_by_key = HashMap::new();

    for record in records {
        let Some(raw_fields) = record.fields.as_object() else {
            continue;
        };

        let mut fields = Map::new();

        for (field_name, raw_value) in raw_fields {
            if rules.should_skip_field(field_name) {
                continue;
            }

            let write_value = rules.to_bitable_write_value(field_name, raw_value.clone());

            if !write_value.is_null() {
                fields.insert(field_name.clone(), write_value);
            }
        }

        let Some(unique_key) =
            find_field_value_by_name(&fields, unique_key_field).and_then(value_to_key_string)
        else {
            continue;
        };

        if let Some(first_record_id) =
            first_record_id_by_key.insert(unique_key.clone(), record.record_id.clone())
        {
            return Err(anyhow!(
                "多维表格来源唯一键字段 `{}` 存在重复值 `{}`：record_id `{}` 与 `{}`",
                unique_key_field,
                unique_key,
                first_record_id,
                record.record_id
            ));
        }

        result.push(SourceRow { unique_key, fields });
    }

    Ok(result)
}

/// 把已有多维表格记录按唯一键建立索引。
pub fn index_existing_records(
    records: &[BitableRecord],
    unique_key_field: &str,
) -> anyhow::Result<HashMap<String, ExistingRecordIndexItem>> {
    let mut index: HashMap<String, ExistingRecordIndexItem> = HashMap::new();

    for record in records {
        let Some(fields) = record.fields.as_object() else {
            continue;
        };

        let Some(unique_value) = find_field_value_by_name(fields, unique_key_field) else {
            continue;
        };

        let Some(unique_key) = value_to_key_string(unique_value) else {
            continue;
        };

        if let Some(existing) = index.get(&unique_key) {
            return Err(anyhow!(
                "目标表唯一键字段 `{}` 存在重复值 `{}`：record_id `{}` 与 `{}`",
                unique_key_field,
                unique_key,
                existing.record_id,
                record.record_id
            ));
        }

        index.insert(
            unique_key,
            ExistingRecordIndexItem {
                record_id: record.record_id.clone(),
                fields: fields.clone(),
            },
        );
    }

    Ok(index)
}

/// 构建主表同步计划。
///
/// 已存在记录只更新变化字段；不存在记录则创建整行字段。
pub fn build_upsert_plan(
    source_rows: Vec<SourceRow>,
    existing_index: HashMap<String, ExistingRecordIndexItem>,
    rules: &FieldValueRules,
) -> UpsertPlan {
    let mut need_create_records = Vec::new();
    let mut need_update_records = Vec::new();

    for row in source_rows {
        match existing_index.get(&row.unique_key) {
            Some(existing) => {
                let changed_fields = diff_fields(&existing.fields, &row.fields, rules);

                if !changed_fields.is_empty() {
                    need_update_records.push(UpdateRecordItem {
                        record_id: existing.record_id.clone(),
                        fields: Value::Object(changed_fields),
                    });
                }
            }
            None => need_create_records.push(new_create_record(row.fields)),
        }
    }

    UpsertPlan {
        need_create_records,
        need_update_records,
    }
}

/// 构建 audit 表需要创建的记录。
///
/// audit 表目前只写一个唯一键字段；如果后续需要同步更多字段，
/// 可以新增一个专门的 audit step，而不是改主表 upsert 逻辑。
pub fn build_missing_key_create_records(
    keys: Vec<String>,
    existing_index: &HashMap<String, ExistingRecordIndexItem>,
    unique_key_field: &str,
    rules: &FieldValueRules,
) -> Vec<CreateRecordItem> {
    let mut need_create_records = Vec::new();

    for key in keys {
        if existing_index.contains_key(&key) {
            continue;
        }

        let mut fields = Map::new();
        fields.insert(
            unique_key_field.to_string(),
            rules.to_bitable_write_value(unique_key_field, Value::String(key)),
        );

        need_create_records.push(CreateRecordItem {
            fields: Value::Object(fields),
        });
    }

    need_create_records
}

/// 解析表头。
fn parse_headers(header_row: Vec<Value>) -> anyhow::Result<Vec<String>> {
    let mut headers = Vec::new();

    for (index, value) in header_row.into_iter().enumerate() {
        let header = value
            .as_str()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .ok_or_else(|| anyhow!("第 {} 列表头非法：{}", index + 1, value))?;

        headers.push(header.to_string());
    }

    Ok(headers)
}

/// 空行判断。
fn is_empty_row(row: &[Value]) -> bool {
    row.iter().all(|value| match value {
        Value::Null => true,
        Value::String(s) => s.trim().is_empty(),
        _ => false,
    })
}

/// 创建接口需要 CreateRecordItem，而不是原始 Record。
fn new_create_record(fields: Map<String, Value>) -> CreateRecordItem {
    CreateRecordItem {
        fields: Value::Object(fields),
    }
}

/// 找出变化字段。
///
/// 注意：这里用 semantic_equal，而不是直接 old_value != new_value。
/// 因为飞书读取的文本字段是 `[{ text, type }]`，而写入时通常是 String。
fn diff_fields(
    old_fields: &Map<String, Value>,
    new_fields: &Map<String, Value>,
    rules: &FieldValueRules,
) -> Map<String, Value> {
    let mut changed = Map::new();

    for (key, new_value) in new_fields {
        if rules.should_skip_field(key) {
            continue;
        }

        let old_value = find_field_value_by_name(old_fields, key);

        let is_same = match old_value {
            Some(old_value) => semantic_equal(old_value, new_value),
            None => false,
        };

        if !is_same {
            changed.insert(key.clone(), new_value.clone());
        }
    }

    changed
}

/// 用归一化字段名查找字段值。
pub(crate) fn find_field_value_by_name<'a>(
    fields: &'a Map<String, Value>,
    field_name: &str,
) -> Option<&'a Value> {
    let target = normalize_field_name(field_name);

    fields
        .iter()
        .find(|(name, _)| normalize_field_name(name) == target)
        .map(|(_, value)| value)
}

/// 语义比较。
fn semantic_equal(old_value: &Value, new_value: &Value) -> bool {
    let old_normalized = normalize_for_compare(old_value);
    let new_normalized = normalize_for_compare(new_value);

    old_normalized == new_normalized
}

/// 用于比较的归一化。
fn normalize_for_compare(value: &Value) -> Value {
    match value {
        Value::Null => Value::Null,
        Value::Bool(_) => value.clone(),
        Value::Number(_) => value.clone(),
        Value::String(s) => {
            let s = clean_text(s);

            if s.is_empty() {
                Value::Null
            } else if let Some(number_value) = parse_string_to_number_value(&s) {
                // 纯数字字符串和数字统一比较，避免 1 和 "1" 被认为不同。
                number_value
            } else {
                Value::String(s)
            }
        }
        Value::Array(items) => {
            let mut parts = Vec::new();

            for item in items {
                if let Some(s) = value_to_plain_string(item) {
                    parts.push(s);
                }
            }

            let text = clean_text(&parts.join(""));

            if text.is_empty() {
                Value::Null
            } else if let Some(number_value) = parse_string_to_number_value(&text) {
                number_value
            } else {
                Value::String(text)
            }
        }
        Value::Object(obj) => {
            // 飞书 URL 字段读取格式一般包含 link/text。
            if let Some(link) = obj.get("link").and_then(Value::as_str) {
                return Value::String(clean_text(link));
            }

            if let Some(text) = obj.get("text").and_then(Value::as_str) {
                return Value::String(clean_text(text));
            }

            // 飞书公式字段读取格式可能是 `{ type, value }`。
            if let Some(inner) = obj.get("value") {
                return normalize_for_compare(inner);
            }

            value.clone()
        }
    }
}

/// 把任意 Value 尽量转成普通字符串。
pub fn value_to_plain_string(value: &Value) -> Option<String> {
    match value {
        Value::Null => None,
        Value::String(s) => {
            let s = clean_text(s);
            if s.is_empty() { None } else { Some(s) }
        }
        Value::Number(n) => Some(n.to_string()),
        Value::Bool(b) => Some(b.to_string()),
        Value::Array(items) => {
            let mut parts = Vec::new();

            for item in items {
                if let Some(s) = value_to_plain_string(item) {
                    parts.push(s);
                }
            }

            let s = clean_text(&parts.join(""));
            if s.is_empty() { None } else { Some(s) }
        }
        Value::Object(obj) => {
            if let Some(text) = obj.get("text").and_then(Value::as_str) {
                return Some(clean_text(text));
            }

            if let Some(link) = obj.get("link").and_then(Value::as_str) {
                return Some(clean_text(link));
            }

            if let Some(name) = obj.get("name").and_then(Value::as_str) {
                return Some(clean_text(name));
            }

            if let Some(inner) = obj.get("value") {
                return value_to_plain_string(inner);
            }

            None
        }
    }
}

/// 唯一键解析。
///
/// 兼容：
/// - `"demo-video-id"`
/// - `123456789`
/// - `[{ "text": "demo-video-id", "type": "text" }]`
/// - `{ "value": [{ "text": "xxx" }] }`
fn value_to_key_string(value: &Value) -> Option<String> {
    value_to_plain_string(value).and_then(|s| {
        let s = s.trim().replace(['\r', '\n', ' ', '\u{3000}'], "");

        if s.is_empty() { None } else { Some(s) }
    })
}

/// 字段名归一化。
///
/// 飞书字段名有时会混入空格、换行或全角空格，统一去掉后再比较。
pub fn normalize_field_name(s: &str) -> String {
    s.trim()
        .replace(['\r', '\n', ' ', '\u{3000}'], "")
        .to_lowercase()
}

/// 批量归一化字段名。
fn normalize_field_names(fields: &[&str]) -> Vec<String> {
    fields
        .iter()
        .map(|field| normalize_field_name(field))
        .collect()
}

/// 单元格清洗。
fn normalize_cell_value(value: Value) -> Value {
    match value {
        Value::String(s) => {
            let s = clean_text(&s);
            if s.is_empty() {
                Value::Null
            } else {
                Value::String(s)
            }
        }
        other => other,
    }
}

/// 文本清洗。
fn clean_text(s: &str) -> String {
    s.trim()
        .replace(['\r', '\n'], " ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// 转数字字段。
fn to_number_value(value: Value) -> Value {
    match value {
        Value::Number(_) => value,
        Value::String(s) => {
            let s = s.trim().replace(',', "");

            if s.is_empty() {
                return Value::Null;
            }

            parse_string_to_number_value(&s).unwrap_or(Value::Null)
        }
        Value::Bool(b) => {
            if b {
                Value::Number(Number::from(1))
            } else {
                Value::Number(Number::from(0))
            }
        }
        other => other,
    }
}

/// 解析字符串为 serde_json::Value::Number。
fn parse_string_to_number_value(s: &str) -> Option<Value> {
    let s = s.trim().replace(',', "");

    if s.is_empty() {
        return None;
    }

    if let Ok(i) = s.parse::<i64>() {
        return Some(Value::Number(Number::from(i)));
    }

    if let Ok(f) = s.parse::<f64>() {
        return Number::from_f64(f).map(Value::Number);
    }

    None
}

/// 转时间戳毫秒。
///
/// 支持：
/// - 已经是数字的毫秒时间戳
/// - 纯数字字符串
/// - `2026-06-01 00:28:25`
/// - `2026/06/01 00:28:25`
/// - `2026-06-01`
fn to_timestamp_millis_value(value: Value) -> Value {
    match value {
        Value::Number(_) => value,
        Value::String(s) => {
            let s = clean_text(&s);

            if s.is_empty() {
                return Value::Null;
            }

            if let Ok(i) = s.parse::<i64>() {
                return Value::Number(Number::from(i));
            }

            if let Ok(dt) = NaiveDateTime::parse_from_str(&s, "%Y-%m-%d %H:%M:%S")
                && let Some(local_dt) = Shanghai.from_local_datetime(&dt).single()
            {
                return Value::Number(Number::from(local_dt.timestamp_millis()));
            }

            if let Ok(dt) = NaiveDateTime::parse_from_str(&s, "%Y/%m/%d %H:%M:%S")
                && let Some(local_dt) = Shanghai.from_local_datetime(&dt).single()
            {
                return Value::Number(Number::from(local_dt.timestamp_millis()));
            }

            if let Ok(dt) =
                NaiveDateTime::parse_from_str(&format!("{} 00:00:00", s), "%Y-%m-%d %H:%M:%S")
                && let Some(local_dt) = Shanghai.from_local_datetime(&dt).single()
            {
                return Value::Number(Number::from(local_dt.timestamp_millis()));
            }

            // 解析不了就不写入，避免飞书字段校验失败。
            Value::Null
        }
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_sheet_rows_rejects_duplicate_normalized_keys_with_row_numbers() {
        let rows = vec![
            vec![json!("视频/图文ID"), json!("标题")],
            vec![json!("video 1"), json!("第一条")],
            vec![json!("video1"), json!("第二条")],
        ];

        let error = parse_sheet_rows(rows, "视频/图文ID", &FieldValueRules::new())
            .expect_err("duplicate Sheet keys must be rejected");
        let message = format!("{error:#}");

        assert!(message.contains("唯一键字段 `视频/图文ID` 存在重复值 `video1`"));
        assert!(message.contains("第 2 行与第 3 行"));
    }

    #[test]
    fn parse_bitable_records_rejects_duplicate_normalized_keys_with_record_ids() {
        let records = vec![
            new_record("rec_manual_1", json!({ "视频/图文ID": "video 1" })),
            new_record("rec_manual_2", json!({ "视频/图文ID": "video1" })),
        ];

        let error =
            parse_bitable_records_as_source_rows(&records, "视频/图文ID", &FieldValueRules::new())
                .expect_err("duplicate manual keys must be rejected");
        let message = format!("{error:#}");

        assert!(message.contains("唯一键字段 `视频/图文ID` 存在重复值 `video1`"));
        assert!(message.contains("record_id `rec_manual_1` 与 `rec_manual_2`"));
    }

    #[test]
    fn index_existing_records_rejects_duplicate_normalized_keys_with_record_ids() {
        let records = vec![
            new_record("rec_target_1", json!({ "视频/图文ID": "video 1" })),
            new_record("rec_target_2", json!({ "视频/图文ID": "video1" })),
        ];

        let error = index_existing_records(&records, "视频/图文ID")
            .expect_err("duplicate target keys must be rejected");
        let message = format!("{error:#}");

        assert!(message.contains("唯一键字段 `视频/图文ID` 存在重复值 `video1`"));
        assert!(message.contains("record_id `rec_target_1` 与 `rec_target_2`"));
    }

    fn new_record(record_id: &str, fields: Value) -> BitableRecord {
        BitableRecord {
            record_id: record_id.to_string(),
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
