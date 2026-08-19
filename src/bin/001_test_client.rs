use lark_exp::client::LarkClient;
use lark_exp::config::Config;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // 1. 读取配置
    let config = Config::from_env()?;

    // 2. 创建应用级 LarkClient
    // 这里内部已经创建了 openlark::Client 和 TokenCache。
    let lark = LarkClient::new(config)?;

    let app_access_token = lark.get_tenant_access_token().await?;
    println!("App Access Token: {:?}", app_access_token);

    Ok(())
}
