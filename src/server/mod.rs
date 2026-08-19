pub mod admin;
pub mod admin_sheet;
pub mod api;
pub mod audit_extra_query;
pub mod auth;
pub mod cache;
pub mod data_protection;
pub mod data_sync;
pub mod error;
pub mod query;
pub mod scheduler;
pub mod secret_store;
pub mod state;

use crate::client::LarkClient;
use crate::config::{Config, DatabaseConfig};
use crate::lark::message_history::CardMessageHistoryRepository;
use crate::server::secret_store::SessionCipher;
use crate::server::state::AppState;
use crate::workflow::XingtuWorkflowService;
use crate::workflow_run::WorkflowRunRepository;
use crate::xingtu::XingtuSession;
use crate::xingtu::account::{XingtuAccountRepository, XingtuSessionRegistry};
use crate::xingtu::activity_config::XingtuActivityConfigRepository;
use crate::xingtu::data_import::XingtuDataImportRepository;
use anyhow::{Context, bail};
use salvo::compression::{Compression, CompressionLevel};
use salvo::cors::{AllowHeaders, AllowOrigin, Cors, ExposeHeaders};
use salvo::http::header::{HeaderName, HeaderValue};
use salvo::http::{Method, mime};
use salvo::oapi::{Info, OpenApi, swagger_ui::SwaggerUi};
use salvo::prelude::*;
use sqlx::Executor;
use sqlx::postgres::PgPoolOptions;
use std::{collections::HashMap, env, sync::Arc};
use url::{Host, Url};

/// 构建服务端共享状态。
pub async fn build_app_state() -> anyhow::Result<Arc<AppState>> {
    dotenvy::dotenv().ok();

    let database_config = DatabaseConfig::from_env()?;
    let pool = PgPoolOptions::new()
        .max_connections(10)
        .after_connect(|conn, _meta| {
            Box::pin(async move {
                // 业务日期全部按北京时间计算；连接建立时固定数据库会话时区，
                // 让 now()/CURRENT_DATE/timestamptz 展示不受数据库默认时区影响。
                conn.execute("SET TIME ZONE 'Asia/Shanghai'").await?;
                Ok::<(), sqlx::Error>(())
            })
        })
        .connect(&database_config.database_url)
        .await
        .context("连接数据库失败")?;
    sqlx::migrate!("./migrations")
        .run(&pool)
        .await
        .context("执行数据库 migration 失败")?;

    let lark = LarkClient::new(Config::from_env()?)?;
    let activity_repo = XingtuActivityConfigRepository::new(pool.clone());
    let data_import_repo = XingtuDataImportRepository::new(pool.clone());
    let account_repo = XingtuAccountRepository::new(pool.clone(), SessionCipher::from_env()?);
    let message_history_repo = CardMessageHistoryRepository::new(pool.clone());
    let workflow_run_repo = WorkflowRunRepository::new(pool.clone());
    let recovered_runs = workflow_run_repo
        .reconcile_stale_runs(std::time::Duration::from_secs(6 * 60 * 60))
        .await
        .context("恢复异常中断的工作流台账失败")?;
    let session_registry = XingtuSessionRegistry::new();
    let workflow = XingtuWorkflowService::new(
        activity_repo,
        data_import_repo,
        account_repo,
        message_history_repo,
        workflow_run_repo,
        session_registry,
        lark,
    );
    let restored_sessions = workflow
        .restore_sessions_from_db()
        .await
        .context("恢复并迁移星图登录态失败")?;
    let env_session_loaded = load_debug_xingtu_session_from_env(&workflow).await?;

    tracing::info!("服务启动时恢复星图登录态：{} 个", restored_sessions);
    if recovered_runs > 0 {
        tracing::warn!(recovered_runs, "已将异常中断的陈旧工作流台账标记为失败");
    }
    if env_session_loaded {
        tracing::info!("已从 .env 注入调试星图登录态");
    }

    Ok(Arc::new(AppState::new(pool, workflow)))
}

/// 构建 Salvo 路由。
pub fn build_router(state: Arc<AppState>) -> anyhow::Result<Router> {
    let cors = build_cors()?.into_handler();
    let data_protection = data_protection::DataProtection::from_env()?;
    let documented_router = Router::new()
        .push(Router::with_path("health").get(api::health::health))
        .push(Router::with_path("live").get(api::health::live))
        .push(Router::with_path("ready").get(api::health::ready))
        .push(api::routes());
    let openapi = OpenApi::with_info(
        Info::new("Lark Xingtu Workflow API", crate::version::version())
            .description("星图活动数据拉取、入库、同步、审核通知和分析查询接口"),
    )
    .merge_router(&documented_router);

    Ok(Router::new()
        .hoop(salvo::affix_state::inject(state))
        .hoop(cors)
        .hoop(build_compression())
        .unshift(openapi.into_router("/api-doc/openapi.json"))
        .unshift(SwaggerUi::new("/api-doc/openapi.json").into_router("/swagger-ui"))
        .push(data_sync::routes())
        .push(admin::ui_routes())
        .push(Router::with_path("health").get(api::health::health))
        .push(Router::with_path("live").get(api::health::live))
        .push(Router::with_path("ready").get(api::health::ready))
        .push(api::routes().hoop(data_protection))
        .push(cors_preflight_route()))
}

fn build_compression() -> Compression {
    Compression::new()
        .enable_gzip(CompressionLevel::Fastest)
        .enable_brotli(CompressionLevel::Fastest)
        .enable_deflate(CompressionLevel::Fastest)
        .enable_zstd(CompressionLevel::Fastest)
        .min_length(1024)
        .content_types(&[
            mime::TEXT_HTML,
            mime::TEXT_CSS,
            mime::TEXT_JAVASCRIPT,
            mime::TEXT_PLAIN,
            mime::TEXT_XML,
            mime::APPLICATION_JAVASCRIPT,
            mime::APPLICATION_JSON,
            "application/xml".parse().expect("valid XML MIME type"),
            mime::IMAGE_SVG,
        ])
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct CorsDomainRule {
    host: String,
    subdomains_only: bool,
}

fn build_cors() -> anyhow::Result<Cors> {
    let rules = cors_domain_rules_from_env()?;
    if rules.is_empty() {
        tracing::warn!("未配置 CORS_DOMAIN，跨域请求不会获得 Access-Control-Allow-Origin");
    } else {
        tracing::info!(rule_count = rules.len(), "已加载 CORS 域名规则");
    }
    Ok(build_cors_with_rules(rules))
}

fn build_cors_with_rules(rules: Vec<CorsDomainRule>) -> Cors {
    let rules = Arc::new(rules);
    Cors::new()
        .allow_origin(AllowOrigin::dynamic(move |origin, _req, _depot| {
            allowed_cors_origin(origin, &rules)
        }))
        .allow_methods(vec![
            Method::GET,
            Method::POST,
            Method::PUT,
            Method::PATCH,
            Method::DELETE,
            Method::OPTIONS,
        ])
        .allow_headers(AllowHeaders::mirror_request())
        .expose_headers(ExposeHeaders::list([
            HeaderName::from_static(data_protection::DATA_PROTECTION_HEADER),
            HeaderName::from_static(error::REQUEST_ID_HEADER),
        ]))
        .max_age(86400)
}

fn cors_domain_rules_from_env() -> anyhow::Result<Vec<CorsDomainRule>> {
    match env::var("CORS_DOMAIN") {
        Ok(value) => parse_cors_domain_rules(&value),
        Err(env::VarError::NotPresent) => Ok(Vec::new()),
        Err(env::VarError::NotUnicode(_)) => bail!("环境变量 CORS_DOMAIN 不是有效 UTF-8"),
    }
}

fn parse_cors_domain_rules(value: &str) -> anyhow::Result<Vec<CorsDomainRule>> {
    let value = value.trim();
    if value.is_empty() {
        bail!("CORS_DOMAIN 已配置但内容为空");
    }

    let entries = if value.starts_with('[') || value.ends_with(']') {
        if !(value.starts_with('[') && value.ends_with(']')) {
            bail!("CORS_DOMAIN 的方括号不完整");
        }
        match serde_json::from_str::<Vec<String>>(value) {
            Ok(entries) => entries,
            Err(_) if !value.contains('\"') && !value.contains('\'') => value[1..value.len() - 1]
                .split(',')
                .map(str::trim)
                .map(str::to_owned)
                .collect(),
            Err(_) => bail!("CORS_DOMAIN JSON 数组格式无效，数组元素必须是字符串"),
        }
    } else {
        value.split(',').map(str::trim).map(str::to_owned).collect()
    };

    if entries.is_empty() {
        bail!("CORS_DOMAIN 至少需要一个域名规则");
    }

    let mut rules = Vec::with_capacity(entries.len());
    for (index, entry) in entries.into_iter().enumerate() {
        let entry = entry.trim();
        if entry.is_empty() {
            bail!("CORS_DOMAIN 第 {} 项为空", index + 1);
        }
        if entry == "*" {
            bail!("CORS_DOMAIN 不允许使用全开放通配符 *");
        }
        if entry.contains("://")
            || entry.contains('/')
            || entry.contains('?')
            || entry.contains('#')
            || entry.contains('@')
            || entry.contains(':')
        {
            bail!(
                "CORS_DOMAIN 第 {} 项只能填写域名或 IPv4 地址，不能包含协议、端口或路径",
                index + 1
            );
        }

        let (subdomains_only, host_text) = entry
            .strip_prefix("*.")
            .map_or((false, entry), |host| (true, host));
        if host_text.is_empty() || host_text.starts_with('.') || host_text.ends_with('.') {
            bail!("CORS_DOMAIN 第 {} 项格式无效", index + 1);
        }

        let host = match Host::parse(host_text)
            .map_err(|_| anyhow::anyhow!("CORS_DOMAIN 第 {} 项不是有效域名", index + 1))?
        {
            Host::Domain(domain) => {
                if subdomains_only && !domain.contains('.') {
                    bail!("CORS_DOMAIN 第 {} 项的子域通配规则过宽", index + 1);
                }
                domain
            }
            Host::Ipv4(address) if !subdomains_only => address.to_string(),
            Host::Ipv4(_) | Host::Ipv6(_) => {
                bail!("CORS_DOMAIN 第 {} 项不能对 IP 地址使用通配符", index + 1)
            }
        };
        let rule = CorsDomainRule {
            host,
            subdomains_only,
        };
        if !rules.contains(&rule) {
            rules.push(rule);
        }
    }

    if rules.is_empty() {
        bail!("CORS_DOMAIN 至少需要一个有效域名规则");
    }
    Ok(rules)
}

fn cors_preflight_route() -> Router {
    Router::with_path("{**cors_preflight_path}").options(cors_preflight)
}

#[handler]
async fn cors_preflight() {}

fn allowed_cors_origin(
    origin: Option<&HeaderValue>,
    rules: &[CorsDomainRule],
) -> Option<HeaderValue> {
    let origin = origin?;
    let origin_text = origin.to_str().ok()?;

    if is_allowed_cors_origin(origin_text, rules) {
        Some(origin.clone())
    } else {
        None
    }
}

fn is_allowed_cors_origin(origin: &str, rules: &[CorsDomainRule]) -> bool {
    let Ok(url) = Url::parse(origin) else {
        return false;
    };
    if !matches!(url.scheme(), "http" | "https") {
        return false;
    }
    if !url.username().is_empty()
        || url.password().is_some()
        || url.path() != "/"
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return false;
    }
    let Some(host) = url.host_str() else {
        return false;
    };

    rules.iter().any(|rule| matches_cors_domain(host, rule))
}

fn matches_cors_domain(host: &str, rule: &CorsDomainRule) -> bool {
    if rule.subdomains_only {
        host != rule.host && host.ends_with(&format!(".{}", rule.host))
    } else {
        host == rule.host
    }
}

async fn load_debug_xingtu_session_from_env(
    workflow: &XingtuWorkflowService,
) -> anyhow::Result<bool> {
    let Ok(cookie) = env::var("XINGTU_COOKIE") else {
        return Ok(false);
    };
    let Ok(csrf_token) = env::var("XINGTU_CSRF_TOKEN") else {
        return Ok(false);
    };

    let account_id = env::var("XINGTU_ACCOUNT_ID")
        .map_err(|_| anyhow::anyhow!("配置调试登录态时必须同时设置 XINGTU_ACCOUNT_ID"))?;
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

    workflow
        .upsert_xingtu_session(&account_id, session)
        .await
        .context("从 .env 注入星图登录态失败")?;

    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use salvo::http::StatusCode;
    use salvo::http::header::{
        ACCEPT_ENCODING, ACCESS_CONTROL_ALLOW_HEADERS, ACCESS_CONTROL_ALLOW_METHODS,
        ACCESS_CONTROL_ALLOW_ORIGIN, ACCESS_CONTROL_EXPOSE_HEADERS, CONTENT_ENCODING, CONTENT_TYPE,
    };
    use salvo::test::TestClient;
    use serde_json::{Value, json};

    #[handler]
    async fn cors_probe() -> &'static str {
        "ok"
    }

    #[handler]
    async fn large_json_probe() -> Json<Value> {
        Json(json!({ "payload": "x".repeat(2048) }))
    }

    #[handler]
    async fn small_json_probe() -> Json<Value> {
        Json(json!({ "ok": true }))
    }

    #[handler]
    async fn protected_response_probe(res: &mut Response) {
        res.headers_mut().insert(
            CONTENT_TYPE,
            HeaderValue::from_static("application/vnd.lark-utils-exp.protected+json"),
        );
        res.body("x".repeat(2048));
    }

    fn cors_test_service() -> Service {
        Service::new(
            Router::new()
                .hoop(build_cors_with_rules(test_cors_rules()).into_handler())
                .push(Router::with_path("probe").get(cors_probe))
                .push(cors_preflight_route()),
        )
    }

    fn test_cors_rules() -> Vec<CorsDomainRule> {
        parse_cors_domain_rules(
            r#"["example.com","*.example.com","another.test","*.another.test"]"#,
        )
        .expect("valid CORS test rules")
    }

    fn compression_test_service() -> Service {
        Service::new(
            Router::new()
                .hoop(build_compression())
                .push(Router::with_path("large-json").get(large_json_probe))
                .push(Router::with_path("small-json").get(small_json_probe))
                .push(Router::with_path("protected").get(protected_response_probe)),
        )
    }

    fn data_protection_scope_test_service() -> Service {
        let data_protection =
            data_protection::DataProtection::with_key_for_test([0x4d; 32], "scope-test");
        Service::new(
            Router::new()
                .push(
                    Router::with_path("api/v1")
                        .hoop(data_protection)
                        .get(small_json_probe),
                )
                .push(Router::with_path("health").get(small_json_probe))
                .push(Router::with_path("api/data-sync/probe").get(small_json_probe)),
        )
    }

    #[test]
    fn cors_origin_filter_accepts_only_configured_domain_trees() {
        let rules = test_cors_rules();
        for origin in [
            "https://example.com",
            "https://app.example.com",
            "https://a.b.example.com:8443",
            "http://another.test",
            "https://admin.another.test",
            "https://a.b.another.test",
        ] {
            assert!(
                is_allowed_cors_origin(origin, &rules),
                "should allow {origin}"
            );
        }

        for origin in [
            "https://evilexample.com",
            "https://example.com.evil.test",
            "https://another.test.evil.example",
            "https://unconfigured.example",
            "ftp://app.example.com",
            "https://user@example.com",
            "https://example.com/path",
            "https://example.com?query=1",
            "null",
        ] {
            assert!(
                !is_allowed_cors_origin(origin, &rules),
                "should reject {origin}"
            );
        }
    }

    #[test]
    fn cors_domain_parser_supports_json_relaxed_and_comma_separated_lists() {
        for value in [
            r#"["example.com","*.example.com"]"#,
            "[example.com,*.example.com]",
            "example.com,*.example.com",
        ] {
            let rules = parse_cors_domain_rules(value).expect("valid CORS domain list");
            assert_eq!(rules.len(), 2);
            assert_eq!(rules[0].host, "example.com");
            assert!(!rules[0].subdomains_only);
            assert!(rules[1].subdomains_only);
        }
    }

    #[test]
    fn cors_domain_parser_rejects_dangerous_or_malformed_rules() {
        for value in [
            "",
            "[]",
            "*",
            "*.com",
            "https://example.com",
            "example.com:443",
            "example.com/path",
            r#"["example.com", 1]"#,
        ] {
            assert!(
                parse_cors_domain_rules(value).is_err(),
                "should reject {value}"
            );
        }
    }

    #[test]
    fn cors_exact_and_wildcard_rules_have_distinct_meanings() {
        let exact = parse_cors_domain_rules("example.com").expect("valid exact rule");
        assert!(is_allowed_cors_origin("https://example.com", &exact));
        assert!(!is_allowed_cors_origin("https://app.example.com", &exact));

        let wildcard = parse_cors_domain_rules("*.example.com").expect("valid wildcard rule");
        assert!(!is_allowed_cors_origin("https://example.com", &wildcard));
        assert!(is_allowed_cors_origin(
            "https://deep.app.example.com",
            &wildcard
        ));
    }

    #[tokio::test]
    async fn cors_response_reflects_allowed_origin_and_rejects_other_origins() {
        let service = cors_test_service();
        let allowed_origin = "https://dashboard.ops.example.com";
        let allowed_response = TestClient::get("http://127.0.0.1/probe")
            .add_header("Origin", allowed_origin, true)
            .send(&service)
            .await;

        assert_eq!(allowed_response.status_code, Some(StatusCode::OK));
        assert_eq!(
            allowed_response
                .headers()
                .get(ACCESS_CONTROL_ALLOW_ORIGIN)
                .and_then(|value| value.to_str().ok()),
            Some(allowed_origin)
        );
        assert!(
            allowed_response
                .headers()
                .get(ACCESS_CONTROL_EXPOSE_HEADERS)
                .and_then(|value| value.to_str().ok())
                .is_some_and(|headers| headers.split(',').any(|name| {
                    name.trim()
                        .eq_ignore_ascii_case(data_protection::DATA_PROTECTION_HEADER)
                }))
        );

        let rejected_response = TestClient::get("http://127.0.0.1/probe")
            .add_header("Origin", "https://example.com.evil.test", true)
            .send(&service)
            .await;
        assert!(
            rejected_response
                .headers()
                .get(ACCESS_CONTROL_ALLOW_ORIGIN)
                .is_none()
        );
    }

    #[tokio::test]
    async fn cors_preflight_returns_requested_method_and_headers() {
        let service = cors_test_service();
        let origin = "https://admin.eu.another.test";
        let response = TestClient::options("http://127.0.0.1/api/v1/queries/projects")
            .add_header("Origin", origin, true)
            .add_header("Access-Control-Request-Method", "PATCH", true)
            .add_header(
                "Access-Control-Request-Headers",
                "authorization,x-custom-header",
                true,
            )
            .send(&service)
            .await;

        assert_eq!(response.status_code, Some(StatusCode::NO_CONTENT));
        assert_eq!(
            response
                .headers()
                .get(ACCESS_CONTROL_ALLOW_ORIGIN)
                .and_then(|value| value.to_str().ok()),
            Some(origin)
        );
        assert!(
            response
                .headers()
                .get(ACCESS_CONTROL_ALLOW_METHODS)
                .and_then(|value| value.to_str().ok())
                .is_some_and(|methods| methods.contains("PATCH"))
        );
        assert_eq!(
            response
                .headers()
                .get(ACCESS_CONTROL_ALLOW_HEADERS)
                .and_then(|value| value.to_str().ok()),
            Some("authorization,x-custom-header")
        );
    }

    #[tokio::test]
    async fn compression_gzips_large_json_response() {
        let response = TestClient::get("http://127.0.0.1/large-json")
            .add_header(ACCEPT_ENCODING, "gzip", true)
            .send(&compression_test_service())
            .await;

        assert_eq!(response.status_code, Some(StatusCode::OK));
        assert_eq!(
            response
                .headers()
                .get(CONTENT_ENCODING)
                .and_then(|value| value.to_str().ok()),
            Some("gzip")
        );
    }

    #[tokio::test]
    async fn compression_skips_small_json_response() {
        let response = TestClient::get("http://127.0.0.1/small-json")
            .add_header(ACCEPT_ENCODING, "gzip", true)
            .send(&compression_test_service())
            .await;

        assert_eq!(response.status_code, Some(StatusCode::OK));
        assert!(response.headers().get(CONTENT_ENCODING).is_none());
    }

    #[tokio::test]
    async fn compression_skips_protected_content_type() {
        let response = TestClient::get("http://127.0.0.1/protected")
            .add_header(ACCEPT_ENCODING, "gzip", true)
            .send(&compression_test_service())
            .await;

        assert_eq!(response.status_code, Some(StatusCode::OK));
        assert!(response.headers().get(CONTENT_ENCODING).is_none());
    }

    #[tokio::test]
    async fn data_protection_is_scoped_to_api_v1_routes() {
        let service = data_protection_scope_test_service();
        let protected_response = TestClient::get("http://127.0.0.1/api/v1")
            .add_header(
                data_protection::DATA_PROTECTION_HEADER,
                data_protection::DATA_PROTECTION_SCHEME,
                true,
            )
            .send(&service)
            .await;
        assert_eq!(protected_response.status_code, Some(StatusCode::OK));
        assert_eq!(
            protected_response
                .headers()
                .get(data_protection::DATA_PROTECTION_HEADER),
            Some(&HeaderValue::from_static(
                data_protection::DATA_PROTECTION_SCHEME
            ))
        );
        assert_eq!(
            protected_response.headers().get(CONTENT_TYPE),
            Some(&HeaderValue::from_static(
                data_protection::PROTECTED_CONTENT_TYPE
            ))
        );

        for path in ["health", "api/data-sync/probe"] {
            let response = TestClient::get(format!("http://127.0.0.1/{path}"))
                .add_header(
                    data_protection::DATA_PROTECTION_HEADER,
                    data_protection::DATA_PROTECTION_SCHEME,
                    true,
                )
                .send(&service)
                .await;
            assert_eq!(response.status_code, Some(StatusCode::OK));
            assert!(
                response
                    .headers()
                    .get(data_protection::DATA_PROTECTION_HEADER)
                    .is_none(),
                "{path} must not apply data protection"
            );
            assert!(
                response
                    .headers()
                    .get(CONTENT_TYPE)
                    .and_then(|value| value.to_str().ok())
                    .is_some_and(|value| value.starts_with("application/json"))
            );
        }
    }
}
