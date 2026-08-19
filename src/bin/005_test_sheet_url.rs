use lark_exp::client::LarkClient;
use lark_exp::config::Config;
use lark_exp::lark::sheets::export::export_values_to_json_and_csv;
use lark_exp::lark::sheets::get_spreadsheet_token;
use open_lark::docs::SheetRange;
use std::env;
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

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let sheet_url =
        env::var("SHEET_TEST_URL").map_err(|_| anyhow::anyhow!("缺少环境变量 SHEET_TEST_URL"))?;
    // 1. 读取配置
    let config = Config::from_env()?;

    // 2. 创建应用级 LarkClient
    let lark = LarkClient::new(config)?;

    // 3. 获取 tenant_access_token
    // let token = lark.get_tenant_access_token().await?;

    // 4. 切分 sheet_url
    let sheet_token = get_spreadsheet_token(&sheet_url)?;

    // 读取范围
    let sheet_token_infos = lark.raw().docs.list_sheet_infos(&sheet_token).await?;
    let first_sheet_info = sheet_token_infos
        .first()
        .ok_or_else(|| anyhow::anyhow!("没有找到任何 sheet 信息"))?;
    let sheet_data = lark
        .raw()
        .docs
        .read_sheet_ranges(
            &sheet_token,
            vec![SheetRange::new(
                &first_sheet_info.sheet_id,
                build_a1_range(first_sheet_info.row_count, first_sheet_info.column_count),
            )],
        )
        .await?;

    let value_range = sheet_data
        .value_ranges
        .first()
        .ok_or_else(|| anyhow::anyhow!("没有找到任何 sheet 信息"))?;
    export_values_to_json_and_csv(&value_range.values, "sheet_data.json", "sheet_data.csv")?;
    println!(
        "Col Name is:\n {:?}\nFist Row is:\n {:?}",
        sheet_data
            .value_ranges
            .first()
            .ok_or_else(|| anyhow::anyhow!("没有找到任何 sheet 信息"))?
            .values
            .first(),
        sheet_data
            .value_ranges
            .first()
            .ok_or_else(|| anyhow::anyhow!("没有找到任何 sheet 信息"))?
            .values[1]
    );
    Ok(())
}
