use moka::future::Cache;
use serde::Serialize;
use serde::de::DeserializeOwned;
use sqlx::{PgPool, Row};
use std::future::Future;
use std::sync::Arc;
use std::time::Duration;

const QUERY_CACHE_TTL: Duration = Duration::from_secs(5 * 60);
const QUERY_CACHE_MAX_COMPRESSED_BYTES: u64 = 128 * 1024 * 1024;
const QUERY_CACHE_ZSTD_LEVEL: i32 = 1;

/// 查询接口缓存。
///
/// 缓存值统一保存为 zstd 压缩后的 JSON，避免大查询结果以 `serde_json::Value` 常驻内存。
/// 只有三类写工作流会改变业务数据，工作流完成后统一调用 `invalidate_all`。
#[derive(Debug, Clone)]
pub struct QueryCache {
    inner: Cache<String, Arc<[u8]>>,
    pool: PgPool,
}

impl QueryCache {
    pub fn new(pool: PgPool) -> Self {
        Self {
            inner: Cache::builder()
                .max_capacity(QUERY_CACHE_MAX_COMPRESSED_BYTES)
                .weigher(|key: &String, value: &Arc<[u8]>| {
                    u32::try_from(key.len().saturating_add(value.len())).unwrap_or(u32::MAX)
                })
                .time_to_live(QUERY_CACHE_TTL)
                .build(),
            pool,
        }
    }

    pub async fn get_or_try_insert<T, Fut, F>(&self, key: String, loader: F) -> anyhow::Result<T>
    where
        T: Serialize + DeserializeOwned + Clone + Send + Sync + 'static,
        Fut: Future<Output = anyhow::Result<T>>,
        F: FnOnce() -> Fut,
    {
        let revision = self.current_revision().await?;
        let versioned_key = format!("{revision}:{key}");
        if let Some(value) = self.inner.get(&versioned_key).await {
            match decode_cache_value(&value) {
                Ok(data) => return Ok(data),
                Err(error) => {
                    tracing::warn!(
                        cache_key = versioned_key,
                        error = ?error,
                        "查询缓存解压或反序列化失败，丢弃后重新加载"
                    );
                    self.inner.invalidate(&versioned_key).await;
                }
            }
        }

        let data = loader().await?;
        match encode_cache_value(&data) {
            Ok(value) => self.inner.insert(versioned_key, value).await,
            Err(error) => {
                // 缓存失败不能把已经成功的业务查询变成 HTTP 失败。
                tracing::warn!(error = ?error, "查询结果压缩缓存失败，本次直接返回数据库结果");
            }
        }
        Ok(data)
    }

    pub fn invalidate_all(&self) {
        self.inner.invalidate_all();
    }

    /// 本地立即失效并递增数据库版本，让其他实例在下一次查询时自动绕过旧缓存。
    pub async fn invalidate_all_shared(&self) -> anyhow::Result<()> {
        sqlx::query(
            "UPDATE query_cache_revision SET revision = revision + 1, updated_at = now() WHERE singleton = true",
        )
        .execute(&self.pool)
        .await?;
        self.invalidate_all();
        Ok(())
    }

    async fn current_revision(&self) -> anyhow::Result<i64> {
        let row = sqlx::query("SELECT revision FROM query_cache_revision WHERE singleton = true")
            .fetch_one(&self.pool)
            .await?;
        Ok(row.try_get("revision")?)
    }
}

fn encode_cache_value<T: Serialize>(value: &T) -> anyhow::Result<Arc<[u8]>> {
    let json = serde_json::to_vec(value)?;
    let compressed = zstd::stream::encode_all(json.as_slice(), QUERY_CACHE_ZSTD_LEVEL)?;
    Ok(Arc::from(compressed))
}

fn decode_cache_value<T: DeserializeOwned>(value: &[u8]) -> anyhow::Result<T> {
    let json = zstd::stream::decode_all(value)?;
    Ok(serde_json::from_slice(&json)?)
}

/// 写操作失败也可能已经产生部分提交；始终尝试共享失效，同时保留原始业务错误。
pub async fn invalidate_after_write<T>(
    cache: &QueryCache,
    result: anyhow::Result<T>,
) -> anyhow::Result<T> {
    let invalidation = cache.invalidate_all_shared().await;
    match result {
        Ok(value) => {
            invalidation?;
            Ok(value)
        }
        Err(error) => {
            if let Err(cache_error) = invalidation {
                tracing::error!(error = ?cache_error, "写操作失败后共享缓存失效也失败");
            }
            Err(error)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::{Deserialize, Serialize};

    #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
    struct CachedRows {
        items: Vec<String>,
    }

    #[test]
    fn compressed_cache_value_round_trips_and_reduces_repetitive_json() {
        let value = CachedRows {
            items: (0..2_000)
                .map(|index| format!("视频-{index:04}-重复业务字段-重复业务字段"))
                .collect(),
        };
        let json = serde_json::to_vec(&value).unwrap();
        let compressed = encode_cache_value(&value).unwrap();
        let decoded: CachedRows = decode_cache_value(&compressed).unwrap();

        assert_eq!(decoded, value);
        assert!(compressed.len() < json.len() / 2);
    }

    #[test]
    fn compressed_cache_rejects_invalid_payload() {
        assert!(decode_cache_value::<CachedRows>(b"not-zstd").is_err());
    }
}
