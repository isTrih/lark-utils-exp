pub mod export;
pub mod meta;
pub mod read;
pub mod rich_text;
pub mod url;
pub mod write;
pub use {
    url::get_spreadsheet_token, url::parse_lark_sheet_url, url::parse_lark_sheet_url_debug,
    url::resolve_sheet_id_from_url,
};

// 将列号转换为列名，例如：
// 1  -> A
// 26 -> Z
// 27 -> AA
// 52 -> AZ
// 53 -> BA
pub fn column_number_to_name(mut column_number: i32) -> String {
    assert!(column_number > 0, "column_number 必须大于 0");

    let mut result = String::new();

    while column_number > 0 {
        column_number -= 1;

        let ch = ((column_number % 26) as u8 + b'A') as char;
        result.insert(0, ch);

        column_number /= 26;
    }

    result
}

// 根据行数和列数构建 A1 范围字符串，例如：
// row_count = 10, column_count = 3
// 返回：A1:C10
pub fn build_a1_range(row_count: i32, column_count: i32) -> String {
    assert!(row_count > 0, "row_count 必须大于 0");
    assert!(column_count > 0, "column_count 必须大于 0");

    let end_column = column_number_to_name(column_count);

    format!("A1:{}{}", end_column, row_count)
}
