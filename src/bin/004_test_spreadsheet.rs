use lark_exp::client::LarkClient;
use lark_exp::config::Config;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // 1. 读取配置
    let config = Config::from_env()?;

    // 2. 创建应用级 LarkClient
    let lark = LarkClient::new(config)?;

    // 3. 获取 tenant_access_token
    let token = lark.get_tenant_access_token().await?;
    println!("tenant_access_token: {}", token);

    // 4. 使用 tenant_access_token 调用飞书 API
    Ok(())
}
