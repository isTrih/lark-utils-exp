use lark_exp::cache::{TokenCache, TokenInfo};
use std::time::Duration;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // 创建一个测试用缓存。
    //
    // 参数含义：
    // - max_capacity = 100，最多缓存 100 个 token
    // - cache_ttl = 10 秒，缓存项最多存在 10 秒
    // - refresh_before_secs = 2，提前 2 秒刷新
    let cache = TokenCache::with_options(100, Duration::from_secs(10), 2);

    // 第一次获取 token。
    //
    // 因为缓存是空的，所以会执行 refresh_fn。
    let token = cache
        .get_valid_tenant_access_token(|| async {
            println!("第一次：缓存不存在，开始刷新 token");

            // 模拟接口返回的 token。
            //
            // mock_token_001:
            // - token 字符串
            //
            // 5:
            // - 表示 5 秒后过期
            Ok(TokenInfo::new("mock_token_001", 5))
        })
        .await?;

    println!("第一次获取到 token: {}", token);

    // 第二次获取 token。
    //
    // 因为 token 还没过期，所以不会执行 refresh_fn。
    let token = cache
        .get_valid_tenant_access_token(|| async {
            println!("如果你看到这行，说明缓存没有命中");

            Ok(TokenInfo::new("mock_token_002", 5))
        })
        .await?;

    println!("第二次获取到 token: {}", token);

    println!("当前缓存数量: {}", cache.entry_count().await);
    Ok(())
}
