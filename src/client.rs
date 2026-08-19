use crate::cache::{TokenCache, TokenInfo};
use crate::config::Config;
use anyhow::Context;
use open_lark::prelude::*;
use reqwest::Method;
use std::time::Duration;

/// 应用级飞书客户端。
///
/// 这个结构体不是 openlark 原始 Client，
/// 而是你自己封装的一层。
///
/// 它负责统一管理：
/// 1. openlark 原始 Client
/// 2. 配置 Config
/// 3. token 内存缓存 TokenCache
///
/// 外部使用时不需要手动创建 TokenCache。
#[derive(Clone)]
pub struct LarkClient {
    raw: Client,
    config: Config,
    token_cache: TokenCache,
    http: reqwest::Client,
}

/// 飞书 OpenAPI 的原始 JSON 响应。
///
/// 管理接口需要把飞书的 `code/data/msg` 信封原样返回，因此这里不使用会抽取 `data`
/// 的 SDK 高层响应模型。
#[derive(Debug)]
pub struct LarkJsonResponse {
    pub status: u16,
    pub body: serde_json::Value,
}

impl LarkClient {
    /// 创建应用级 LarkClient。
    ///
    /// 使用方式：
    ///
    /// ```rust,no_run
    /// use lark_exp::client::LarkClient;
    /// use lark_exp::config::Config;
    ///
    /// # fn main() -> anyhow::Result<()> {
    /// let config = Config::from_env()?;
    /// let lark = LarkClient::new(config)?;
    /// # Ok(())
    /// # }
    /// ```
    pub fn new(config: Config) -> anyhow::Result<Self> {
        let raw = Client::builder()
            .app_id(&config.lark_app_id)
            .app_secret(&config.lark_app_secret)
            .base_url(&config.lark_base_url)
            .timeout(Duration::from_secs(60))
            .build()?;

        let token_cache = TokenCache::new();
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(60))
            .build()
            .context("创建飞书 HTTP 客户端失败")?;

        Ok(Self {
            raw,
            config,
            token_cache,
            http,
        })
    }

    /// 获取 openlark 原始 Client 引用。
    ///
    /// 后续如果你有些接口暂时没有封装，
    /// 可以通过 raw() 直接访问原始 SDK。
    pub fn raw(&self) -> &Client {
        &self.raw
    }

    /// 获取可用的 tenant_access_token。
    ///
    /// 外部不需要关心缓存。
    ///
    /// 内部流程：
    /// 1. 先从 token_cache 读取
    /// 2. 如果 token 存在且没有过期，直接返回
    /// 3. 如果不存在或快过期，请求飞书刷新
    /// 4. 刷新成功后写入 token_cache
    /// 5. 返回 token 字符串
    pub async fn get_tenant_access_token(&self) -> anyhow::Result<String> {
        self.token_cache
            .get_valid_tenant_access_token(|| async {
                tracing::info!("缓存未命中，开始请求飞书 tenant_access_token");

                let response = self
                    .raw
                    .auth
                    .app
                    .v3()
                    .tenant_access_token_internal()
                    .app_id(&self.config.lark_app_id)
                    .app_secret(&self.config.lark_app_secret)
                    .execute()
                    .await?;

                let access_token = response.data.tenant_access_token;
                let expires_in_secs = response.data.expires_in;

                Ok(TokenInfo::new(access_token, expires_in_secs))
            })
            .await
    }

    /// 手动清理 tenant_access_token。
    ///
    /// 适合接口返回 token 失效、权限变更、调试时主动刷新。
    pub async fn clear_tenant_access_token(&self) {
        self.token_cache
            .remove(crate::cache::LARK_TENANT_ACCESS_TOKEN_KEY)
            .await;
    }

    /// 调试用：查看当前缓存数量。
    pub async fn cache_entry_count(&self) -> u64 {
        self.token_cache.entry_count().await
    }

    /// 使用应用身份调用飞书 GET OpenAPI，并保留官方完整 JSON 信封和 HTTP 状态码。
    ///
    /// `path_segments` 必须逐段传入，避免路径参数被解释成额外路径；查询参数交给 URL
    /// 编码器处理。该方法只供服务端固定路由使用，不接受客户端提供的任意上游 URL。
    pub async fn get_openapi_json(
        &self,
        path_segments: &[&str],
        query: &[(&str, String)],
    ) -> anyhow::Result<LarkJsonResponse> {
        let access_token = self.get_tenant_access_token().await?;
        self.request_openapi_json_with_access_token(
            Method::GET,
            path_segments,
            query,
            None,
            &access_token,
        )
        .await
    }

    /// 使用应用身份调用飞书 POST OpenAPI，并保留官方完整 JSON 信封和 HTTP 状态码。
    pub async fn post_openapi_json(
        &self,
        path_segments: &[&str],
        query: &[(&str, String)],
        body: &serde_json::Value,
    ) -> anyhow::Result<LarkJsonResponse> {
        let access_token = self.get_tenant_access_token().await?;
        self.request_openapi_json_with_access_token(
            Method::POST,
            path_segments,
            query,
            Some(body),
            &access_token,
        )
        .await
    }

    async fn request_openapi_json_with_access_token(
        &self,
        method: Method,
        path_segments: &[&str],
        query: &[(&str, String)],
        body: Option<&serde_json::Value>,
        access_token: &str,
    ) -> anyhow::Result<LarkJsonResponse> {
        let url = build_openapi_url(&self.config.lark_base_url, path_segments, query)?;
        let mut request = self.http.request(method, url).bearer_auth(access_token);
        if let Some(body) = body {
            request = request.json(body);
        }
        let response = request.send().await.context("请求飞书 OpenAPI 失败")?;
        let status = response.status().as_u16();
        let body = response
            .json::<serde_json::Value>()
            .await
            .context("飞书 OpenAPI 返回了非 JSON 响应")?;

        Ok(LarkJsonResponse { status, body })
    }
}

fn build_openapi_url(
    base_url: &str,
    path_segments: &[&str],
    query: &[(&str, String)],
) -> anyhow::Result<reqwest::Url> {
    let mut url = reqwest::Url::parse(base_url).context("LARK_BASE_URL 不是有效 URL")?;
    {
        let mut segments = url
            .path_segments_mut()
            .map_err(|_| anyhow::anyhow!("LARK_BASE_URL 不能作为 API 基础地址"))?;
        segments.pop_if_empty();
        segments.push("open-apis");
        segments.extend(path_segments.iter().copied());
    }
    if !query.is_empty() {
        url.query_pairs_mut()
            .extend_pairs(query.iter().map(|(key, value)| (*key, value.as_str())));
    }
    Ok(url)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn openapi_url_encodes_path_and_query_values() {
        let url = build_openapi_url(
            "https://open.feishu.cn",
            &["im", "v1", "chats", "oc_test/id", "members"],
            &[("page_token", "token+/=".to_owned())],
        )
        .unwrap();

        assert_eq!(
            url.as_str(),
            "https://open.feishu.cn/open-apis/im/v1/chats/oc_test%2Fid/members?page_token=token%2B%2F%3D"
        );
    }

    #[test]
    fn openapi_url_keeps_configured_base_path() {
        let url =
            build_openapi_url("http://127.0.0.1:8080/mock/", &["im", "v1", "chats"], &[]).unwrap();

        assert_eq!(
            url.as_str(),
            "http://127.0.0.1:8080/mock/open-apis/im/v1/chats"
        );
    }
}
