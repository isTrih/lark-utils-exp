use lark_exp::xingtu::{ExportTaskOptions, XingtuClient, XingtuSession};
use std::{collections::HashMap, env};

/// 008：测试“星图任务 ID -> 飞书 spreadsheet URL”。
///
/// 使用方式：
/// `cargo run --bin 008_test_xingtu_export -- <星图任务ID>`
///
/// 当前用环境变量模拟启动时注入登录态：
/// - `XINGTU_COOKIE`
/// - `XINGTU_CSRF_TOKEN`
/// - `XINGTU_SESSION_KEY` 可选
/// - `XINGTU_USER_AGENT` 可选
#[tokio::main]
async fn main() -> anyhow::Result<()> {
    dotenvy::dotenv().ok();

    let task_id = env::args().nth(1).ok_or_else(|| {
        anyhow::anyhow!("请传入星图任务 ID，例如：cargo run --bin 008_test_xingtu_export -- 123456")
    })?;

    let xingtu = XingtuClient::new()?;
    xingtu.set_session(build_session_from_env()?).await?;

    println!("已注入星图登录态，开始导出任务：{}", task_id);

    let result = xingtu
        .export_task_spreadsheet_urls(task_id, ExportTaskOptions::default())
        .await?;

    println!("星图导出完成");
    println!("ticket_id: {}", result.ticket_id);
    println!("polls: {}", result.polls);
    println!("logid: {:?}", result.logid);
    println!("spreadsheet_urls:");

    for url in &result.spreadsheet_urls {
        println!("{}", url);
    }

    Ok(())
}

/// 从环境变量构造登录态。
///
/// 这只是 008 的测试注入方式；真正接调度器或服务接口时，
/// 可以直接构造 `XingtuSession` 后调用 `XingtuClient::set_session`。
fn build_session_from_env() -> anyhow::Result<XingtuSession> {
    let cookie =
        env::var("XINGTU_COOKIE").map_err(|_| anyhow::anyhow!("缺少环境变量 XINGTU_COOKIE"))?;
    let csrf_token = env::var("XINGTU_CSRF_TOKEN")
        .map_err(|_| anyhow::anyhow!("缺少环境变量 XINGTU_CSRF_TOKEN"))?;

    let mut session = XingtuSession::new(cookie, csrf_token);
    session.session_key = env::var("XINGTU_SESSION_KEY")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty());
    session.user_agent = env::var("XINGTU_USER_AGENT")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty());
    session.extra_headers = HashMap::new();

    Ok(session)
}
