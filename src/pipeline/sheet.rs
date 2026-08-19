use crate::client::LarkClient;
use crate::lark::sheets::build_a1_range;
use anyhow::{Context, anyhow};
use open_lark::docs::ccm::sheets_v2::v2::data_io::{ReadSingleRangeParams, read_single_range};
use serde_json::Value;

/// 读取电子表格第一个 Sheet 的全部数据。
///
/// 这个函数只负责“从飞书 Sheet 取原始二维数组”，不做业务字段转换。
/// 这样视频同步、直播同步都可以复用同一个读取步骤。
pub async fn read_first_sheet_rows(
    lark: &LarkClient,
    spreadsheet_token: &str,
) -> anyhow::Result<Vec<Vec<Value>>> {
    let sheet_infos = lark
        .raw()
        .docs
        .list_sheet_infos(spreadsheet_token)
        .await
        .context("读取 sheet infos 失败")?;

    let first_sheet_info = sheet_infos
        .first()
        .ok_or_else(|| anyhow!("没有找到任何 sheet 信息"))?;

    let a1_range = build_a1_range(first_sheet_info.row_count, first_sheet_info.column_count);

    // 飞书 Sheets v2 读取范围需要 `sheet_id!A1:Z1000` 这种格式。
    let value_range = format!("{}!{}", first_sheet_info.sheet_id, a1_range);
    tracing::info!("读取范围：{}", value_range);

    let resp = read_single_range(
        lark.raw().config(),
        spreadsheet_token,
        ReadSingleRangeParams {
            value_range,
            value_render_option: None,
            date_render_option: None,
        },
    )
    .await
    .context("读取 sheet range 失败")?;

    let data = resp
        .data
        .ok_or_else(|| anyhow!("读取 sheet range 成功，但响应 data 为空"))?;

    Ok(data.values)
}
