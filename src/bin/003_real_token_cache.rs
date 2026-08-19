use lark_exp::client::LarkClient;
use lark_exp::config::Config;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // 1. 读取配置
    let config = Config::from_env()?;

    // 2. 创建应用级 LarkClient
    //
    // 注意：
    // 这里内部已经创建了 openlark::Client 和 TokenCache。
    let lark = LarkClient::new(config)?;

    // 3. 第一次获取 token
    //
    // 缓存为空，会请求飞书。
    let token = lark.get_tenant_access_token().await?;
    println!("第一次 tenant_access_token: {}", token);

    // 4. 第二次获取 token
    //
    // 如果没有过期，会直接走缓存。
    let token_again = lark.get_tenant_access_token().await?;
    println!("第二次 tenant_access_token: {}", token_again);

    // 5. 查看缓存数量
    println!("当前缓存数量: {}", lark.cache_entry_count().await);

    Ok(())
}
