use std::env;

#[derive(Debug, Clone)]
pub struct Config {
    pub lark_app_id: String,
    pub lark_app_secret: String,
    pub lark_base_url: String,
}

impl Config {
    pub fn from_env() -> anyhow::Result<Self> {
        dotenvy::dotenv().ok();

        let lark_app_id =
            env::var("LARK_APP_ID").map_err(|_| anyhow::anyhow!("缺少环境变量 LARK_APP_ID"))?;

        let lark_app_secret = env::var("LARK_APP_SECRET")
            .map_err(|_| anyhow::anyhow!("缺少环境变量 LARK_APP_SECRET"))?;

        let lark_base_url =
            env::var("LARK_BASE_URL").unwrap_or_else(|_| "https://open.feishu.cn".to_string());

        Ok(Self {
            lark_app_id,
            lark_app_secret,
            lark_base_url,
        })
    }
}

#[derive(Debug, Clone)]
pub struct DatabaseConfig {
    pub database_url: String,
}

impl DatabaseConfig {
    /// 从环境变量读取数据库连接。
    ///
    /// 调试阶段使用 `.env` 中的 `DATABASE_URL`；后续接企业后端时，
    /// 可以改成由后端配置中心或密钥系统注入。
    pub fn from_env() -> anyhow::Result<Self> {
        dotenvy::dotenv().ok();

        let database_url =
            env::var("DATABASE_URL").map_err(|_| anyhow::anyhow!("缺少环境变量 DATABASE_URL"))?;

        Ok(Self { database_url })
    }
}
