use moka::future::Cache;
use std::future::Future;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// 飞书 tenant_access_token 的缓存 key。
///
/// 用常量统一管理 key，避免在业务代码里到处手写字符串。
pub const LARK_TENANT_ACCESS_TOKEN_KEY: &str = "lark:tenant_access_token";

/// 飞书 app_access_token 的缓存 key。
pub const LARK_APP_ACCESS_TOKEN_KEY: &str = "lark:app_access_token";

/// Token 信息。
///
/// 不建议只缓存一个 String，因为 token 通常还会带有过期时间。
///
/// access_token:
/// - 真正用于请求接口的令牌。
///
/// expire_at:
/// - 令牌过期时间。
/// - 使用 Unix 秒级时间戳。
/// - 例如当前时间是 1000，expires_in 是 7200，则 expire_at = 8200。
#[derive(Debug, Clone)]
pub struct TokenInfo {
    pub access_token: String,
    pub expire_at: u64,
}

impl TokenInfo {
    /// 创建 TokenInfo。
    ///
    /// 参数：
    /// - access_token: 令牌字符串
    /// - expires_in_secs: 多少秒后过期，通常来自接口返回的 expire 字段
    ///
    /// 注意：
    /// - 飞书 token 通常会返回一个有效期字段。
    /// - 这里把“相对过期秒数”转换成“绝对过期时间戳”。
    pub fn new(access_token: impl Into<String>, expires_in_secs: u64) -> Self {
        let now = current_timestamp();

        Self {
            access_token: access_token.into(),
            expire_at: now + expires_in_secs,
        }
    }

    /// 判断 token 是否已经过期。
    ///
    /// 这里不是等到真正过期才返回 true，而是提前一段时间认为它过期。
    /// 这样可以避免以下情况：
    ///
    /// 1. 当前拿到 token 时还没过期
    /// 2. 但真正发请求时 token 已经过期
    /// 3. 接口返回鉴权失败
    ///
    /// refresh_before_secs:
    /// - 提前多少秒刷新。
    /// - 推荐 60 到 300 秒。
    pub fn is_expired(&self, refresh_before_secs: u64) -> bool {
        let now = current_timestamp();

        now >= self.expire_at - refresh_before_secs
    }
}

/// 统一令牌缓存。
///
/// 底层使用 moka::future::Cache，适合 async 场景。
///
/// 为什么不用 RwLock<HashMap>：
/// - RwLock<HashMap> 可以用，但 TTL、容量限制、淘汰策略、并发加载都要自己写。
/// - moka 已经内置容量限制、TTL、并发缓存访问等能力。
#[derive(Debug, Clone)]
pub struct TokenCache {
    inner: Cache<String, TokenInfo>,

    /// 提前刷新时间，单位：秒。
    ///
    /// 例如设置为 300，表示 token 还有 5 分钟过期时，就认为它需要刷新。
    refresh_before_secs: u64,
}

impl TokenCache {
    /// 创建默认 TokenCache。
    ///
    /// 默认策略：
    /// - 最多缓存 100 个 token
    /// - 缓存项最多保留 2 小时
    /// - token 提前 5 分钟刷新
    ///
    /// 注意：
    /// - moka 的 TTL 是缓存层面的兜底过期。
    /// - TokenInfo.expire_at 是业务层面的真实过期判断。
    /// - 两者都保留，是为了更稳。
    pub fn new() -> Self {
        Self::with_options(100, Duration::from_secs(2 * 60 * 60), 5 * 60)
    }

    /// 自定义 TokenCache。
    ///
    /// max_capacity:
    /// - 最大缓存条目数。
    ///
    /// cache_ttl:
    /// - 缓存项在 moka 中的最大存活时间。
    /// - 这是兜底 TTL，不等于 token 的真实有效期。
    ///
    /// refresh_before_secs:
    /// - 提前多少秒刷新 token。
    pub fn with_options(max_capacity: u64, cache_ttl: Duration, refresh_before_secs: u64) -> Self {
        let inner = Cache::builder()
            .max_capacity(max_capacity)
            .time_to_live(cache_ttl)
            .build();

        Self {
            inner,
            refresh_before_secs,
        }
    }

    /// 直接获取缓存中的 TokenInfo。
    ///
    /// 这个方法只负责读缓存，不判断是否过期。
    pub async fn get(&self, key: &str) -> Option<TokenInfo> {
        self.inner.get(key).await
    }

    /// 写入 token。
    ///
    /// 适合你已经手动获取到 token 后，主动塞入缓存。
    pub async fn set(&self, key: impl Into<String>, token: TokenInfo) {
        self.inner.insert(key.into(), token).await;
    }

    /// 删除某个 token。
    ///
    /// 适合 token 明确失效、鉴权失败后主动清理。
    pub async fn remove(&self, key: &str) {
        self.inner.invalidate(key).await;
    }

    /// 清空所有 token。
    ///
    /// 实验仓库中调试很方便。
    pub async fn clear(&self) {
        self.inner.invalidate_all();
    }

    /// 获取当前缓存条目数量。
    ///
    /// 注意：
    /// moka 的 entry_count 不是强一致实时值。
    /// 在读取数量前调用 run_pending_tasks，可以让内部维护任务先执行一轮，
    /// 这样调试时看到的数量会更符合预期。
    pub async fn entry_count(&self) -> u64 {
        self.inner.run_pending_tasks().await;
        self.inner.entry_count()
    }

    /// 获取一个可用 token。
    ///
    /// 这是最核心的方法。
    ///
    /// 执行流程：
    ///
    /// 1. 先从缓存中读取 token
    /// 2. 如果存在且未过期，直接返回 token 字符串
    /// 3. 如果不存在或快过期，刷新 token
    /// 4. 刷新成功后写入缓存
    /// 5. 返回新的 token 字符串
    ///
    /// refresh_fn:
    /// - 由调用方传入。
    /// - 这样 TokenCache 不依赖具体 SDK。
    /// - 你可以用它刷新 tenant_access_token，也可以刷新 app_access_token。
    ///
    /// 为什么使用 try_get_with：
    /// - 当缓存 key 不存在时，try_get_with 可以负责异步加载并写入。
    /// - 比手写 RwLock<HashMap> 更适合并发场景。
    pub async fn get_valid_token<F, Fut>(&self, key: &str, refresh_fn: F) -> anyhow::Result<String>
    where
        F: FnOnce() -> Fut,
        Fut: Future<Output = anyhow::Result<TokenInfo>>,
    {
        let cache_key = key.to_string();

        // 第一步：先尝试读取缓存。
        //
        // 如果 token 存在，并且没有到提前刷新时间，直接返回。
        if let Some(token) = self.inner.get(&cache_key).await {
            if !token.is_expired(self.refresh_before_secs) {
                return Ok(token.access_token);
            }

            // 如果 token 已过期或接近过期，先主动删除。
            //
            // 这样后面的 try_get_with 会进入刷新逻辑。
            self.inner.invalidate(&cache_key).await;
        }

        // 第二步：缓存没有可用 token，执行刷新逻辑。
        //
        // try_get_with 的意义：
        // - 如果这个 key 已经被其他并发任务刷新并写入了，当前任务可以复用结果。
        // - 如果没有，就执行 async move 里的 refresh_fn。
        //
        // 注意：
        // - try_get_with 返回的错误在 moka 内部可能会被 Arc 包装。
        // - 所以这里用 map_err 转成 anyhow::Error。
        let token = self
            .inner
            .try_get_with(cache_key, async move {
                let new_token = refresh_fn().await?;

                Ok::<TokenInfo, anyhow::Error>(new_token)
            })
            .await
            .map_err(|err| anyhow::anyhow!("刷新 token 失败: {err}"))?;

        Ok(token.access_token)
    }

    /// 获取 tenant_access_token。
    ///
    /// 这是对 get_valid_token 的业务语义封装。
    /// 好处是外部不用关心 key 写什么。
    pub async fn get_valid_tenant_access_token<F, Fut>(
        &self,
        refresh_fn: F,
    ) -> anyhow::Result<String>
    where
        F: FnOnce() -> Fut,
        Fut: Future<Output = anyhow::Result<TokenInfo>>,
    {
        self.get_valid_token(LARK_TENANT_ACCESS_TOKEN_KEY, refresh_fn)
            .await
    }

    /// 获取 app_access_token。
    pub async fn get_valid_app_access_token<F, Fut>(&self, refresh_fn: F) -> anyhow::Result<String>
    where
        F: FnOnce() -> Fut,
        Fut: Future<Output = anyhow::Result<TokenInfo>>,
    {
        self.get_valid_token(LARK_APP_ACCESS_TOKEN_KEY, refresh_fn)
            .await
    }
}

impl Default for TokenCache {
    fn default() -> Self {
        Self::new()
    }
}

/// 获取当前 Unix 秒级时间戳。
///
/// 这里单独封装是为了：
/// - 避免到处写 SystemTime 代码
/// - 后续如果你要做测试，可以更容易替换时间来源
fn current_timestamp() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("系统时间异常：当前时间早于 UNIX_EPOCH")
        .as_secs()
}
