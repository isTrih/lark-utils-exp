use crate::client::LarkClient;
use url::Url;

/// 解析飞书表格链接
/// 默认返回 (spreadsheet_token, sheet_id)
/// 例如：
/// - 输入: "https://example.larksuite.com/sheets/ExampleSpreadsheetToken?sheet=Sheet01"
/// - 输出: ("ExampleSpreadsheetToken", Some("Sheet01"))
/// - 输入: "https://example.larksuite.com/sheets/ExampleSpreadsheetToken"
/// - 输出: ("ExampleSpreadsheetToken", None)
pub fn parse_lark_sheet_url(input: &str) -> anyhow::Result<(String, Option<String>)> {
    let url = Url::parse(input)?;

    let segments: Vec<_> = url
        .path_segments()
        .ok_or_else(|| anyhow::anyhow!("URL 路径不合法"))?
        .collect();

    match url.scheme() {
        "https" | "http" => {}
        _ => anyhow::bail!("URL 协议必须是 http 或 https"),
    }

    if segments.len() < 2 || segments[0] != "sheets" {
        anyhow::bail!("不是有效的飞书表格链接");
    }

    if segments[1].is_empty() {
        anyhow::bail!("飞书表格链接缺少表格 token");
    }

    let spreadsheet_token = segments[1].to_string();

    let sheet_id = url.query_pairs().find_map(|(key, value)| {
        if key == "sheet" {
            Some(value.to_string())
        } else {
            None
        }
    });

    Ok((spreadsheet_token, sheet_id))
}

/// 调试版：解析飞书表格链接
///
/// 返回：
/// - spreadsheet_token
/// - sheet_id: URL 中 ?sheet=xxx 指定的 sheet_id
pub fn parse_lark_sheet_url_debug(input: &str) -> anyhow::Result<(String, Option<String>)> {
    println!("================ parse_lark_sheet_url_debug ================");
    println!("原始输入 input: {}", input);

    let url = Url::parse(input)?;
    println!("URL 解析成功");
    println!("scheme: {}", url.scheme());
    println!("host: {:?}", url.host_str());
    println!("path: {}", url.path());
    println!("query: {:?}", url.query());
    println!("fragment: {:?}", url.fragment());

    match url.scheme() {
        "https" | "http" => {
            println!("协议检查通过: {}", url.scheme());
        }
        other => {
            println!("协议检查失败: {}", other);
            anyhow::bail!("URL 协议必须是 http 或 https");
        }
    }

    let segments: Vec<_> = url
        .path_segments()
        .ok_or_else(|| anyhow::anyhow!("URL 路径不合法"))?
        .collect();

    println!("path segments 数量: {}", segments.len());
    println!("path segments: {:?}", segments);

    for (index, segment) in segments.iter().enumerate() {
        println!("segment[{}] = {:?}", index, segment);
    }

    if segments.len() < 2 {
        println!("路径段数量不足，至少需要 /sheets/{{spreadsheet_token}}");
        anyhow::bail!("不是有效的飞书表格链接：路径段数量不足");
    }

    if segments[0] != "sheets" {
        println!(
            "资源类型不匹配，segments[0] = {:?}，期望是 \"sheets\"",
            segments[0]
        );
        anyhow::bail!("不是有效的飞书表格链接");
    }

    println!("资源类型检查通过: {}", segments[0]);

    if segments[1].is_empty() {
        println!("spreadsheet_token 为空");
        anyhow::bail!("飞书表格链接缺少表格 token");
    }

    let spreadsheet_token = segments[1].to_string();
    println!("提取到 spreadsheet_token: {}", spreadsheet_token);

    println!("开始解析 query 参数");

    for (key, value) in url.query_pairs() {
        println!("query 参数: {} = {}", key, value);
    }

    let sheet_id = url.query_pairs().find_map(|(key, value)| {
        if key == "sheet" {
            Some(value.to_string())
        } else {
            None
        }
    });

    match &sheet_id {
        Some(value) => println!("提取到 sheet_id: {}", value),
        None => println!("URL 中没有指定 sheet_id，也就是没有 ?sheet=xxx"),
    }

    println!("最终结果:");
    println!("spreadsheet_token = {}", spreadsheet_token);
    println!("sheet_id = {:?}", sheet_id);
    println!("============================================================");

    Ok((spreadsheet_token, sheet_id))
}

pub async fn resolve_sheet_id_from_url(
    lark: &LarkClient,
    input: &str,
) -> anyhow::Result<(String, String)> {
    let (sheet_token, sheet_id) = parse_lark_sheet_url(input)?;

    let sheet_id = if let Some(sheet_id) = sheet_id {
        sheet_id
    } else {
        let spreadsheet_sheet_infos = lark.raw().docs.list_sheet_infos(&sheet_token).await?;
        let first_sheet_info = spreadsheet_sheet_infos
            .first()
            .ok_or_else(|| anyhow::anyhow!("没有找到任何 sheet_info"))?;

        first_sheet_info.sheet_id.clone()
    };

    Ok((sheet_token, sheet_id))
}

pub fn get_spreadsheet_token(input: &str) -> anyhow::Result<String> {
    let url = Url::parse(input)?;

    let segments: Vec<_> = url
        .path_segments()
        .ok_or_else(|| anyhow::anyhow!("URL 路径不合法"))?
        .collect();

    match url.scheme() {
        "https" | "http" => {}
        _ => anyhow::bail!("URL 协议必须是 http 或 https"),
    }

    if segments[0].is_empty() {
        anyhow::bail!("飞书表格链接缺少表格 token");
    }

    let spreadsheet_token = segments[1].to_string();

    Ok(spreadsheet_token)
}
