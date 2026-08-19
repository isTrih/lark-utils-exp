use crate::lark::im::{MessageReceiver, parse_receive_id_type};
use crate::server::secret_store::SessionCipher;
use crate::xingtu::XingtuSession;
use anyhow::{Context, anyhow};
use chrono::{DateTime, Utc};
use serde::Serialize;
use serde_json::Value;
use sqlx::{PgPool, Row};
use std::{collections::HashMap, sync::Arc};
use tokio::sync::RwLock;

/// 数据库中的项目星图账号。
#[derive(Debug, Clone, Serialize)]
pub struct XingtuProjectAccount {
    pub xingtu_account_id: String,
    pub project_id: i64,
    pub project: String,
    pub project_display_name: String,
    pub display_name: Option<String>,
    pub receive_id_type: String,
    pub receive_id: String,
    pub ops_ids: Vec<String>,
    pub login_notice_card_template_id: String,
    pub login_check_enabled: bool,
    pub session_status: String,
    pub last_checked_at: Option<DateTime<Utc>>,
    pub last_valid_at: Option<DateTime<Utc>>,
    pub last_invalid_at: Option<DateTime<Utc>>,
    pub last_error: Option<String>,
}

impl XingtuProjectAccount {
    /// 组装登录态失效通知的消息接收者。
    pub fn message_receiver(&self) -> anyhow::Result<MessageReceiver> {
        Ok(MessageReceiver {
            receive_id_type: parse_receive_id_type(&self.receive_id_type)?,
            receive_id: self.receive_id.clone(),
            uuid: None,
        })
    }

    /// 卡片模板中使用的项目名称。
    pub fn project_name(&self) -> String {
        self.project_display_name.clone()
    }

    /// 卡片模板变量要求 ops_ids 是逗号分隔文本。
    pub fn ops_ids_text(&self) -> String {
        self.ops_ids
            .iter()
            .map(|value| value.trim())
            .filter(|value| !value.is_empty())
            .collect::<Vec<_>>()
            .join(",")
    }
}

/// 数据库中保存的星图登录态。
#[derive(Debug, Clone, Serialize)]
pub struct StoredXingtuSession {
    pub xingtu_account_id: String,
    pub session: XingtuSession,
}

/// 星图账号和登录态仓储。
#[derive(Debug, Clone)]
pub struct XingtuAccountRepository {
    pool: PgPool,
    cipher: SessionCipher,
}

impl XingtuAccountRepository {
    pub fn new(pool: PgPool, cipher: SessionCipher) -> Self {
        Self { pool, cipher }
    }

    /// 写入或更新星图登录态。
    ///
    /// 登录态以 AES-256-GCM 密文落库，明文字段仅用于启动时迁移历史数据。
    pub async fn upsert_session(
        &self,
        xingtu_account_id: &str,
        session: &XingtuSession,
    ) -> anyhow::Result<()> {
        if xingtu_account_id.trim().is_empty() {
            return Err(anyhow!("xingtu_account_id 不能为空"));
        }

        let encrypted_payload = self
            .cipher
            .encrypt_session(xingtu_account_id.trim(), session)?;

        sqlx::query(
            r#"
            INSERT INTO xingtu_login_session (
                xingtu_account_id,
                cookie,
                csrf_token,
                session_key,
                user_agent,
                extra_headers,
                encrypted_payload,
                encryption_key_id,
                received_at
            )
            VALUES ($1, NULL, NULL, NULL, NULL, '{}'::jsonb, $2, $3, now())
            ON CONFLICT (xingtu_account_id)
            DO UPDATE SET
                cookie = NULL,
                csrf_token = NULL,
                session_key = NULL,
                user_agent = NULL,
                extra_headers = '{}'::jsonb,
                encrypted_payload = EXCLUDED.encrypted_payload,
                encryption_key_id = EXCLUDED.encryption_key_id,
                received_at = now()
            "#,
        )
        .bind(xingtu_account_id.trim())
        .bind(encrypted_payload)
        .bind(self.cipher.key_id())
        .execute(&self.pool)
        .await
        .with_context(|| format!("写入星图登录态失败：{xingtu_account_id}"))?;

        Ok(())
    }

    /// 读取所有已保存登录态，服务启动时用于恢复内存客户端。
    pub async fn list_stored_sessions(&self) -> anyhow::Result<Vec<StoredXingtuSession>> {
        let rows = sqlx::query(
            r#"
            SELECT
                xingtu_account_id,
                cookie,
                csrf_token,
                session_key,
                user_agent,
                extra_headers,
                encrypted_payload,
                encryption_key_id
            FROM xingtu_login_session
            "#,
        )
        .fetch_all(&self.pool)
        .await
        .context("读取星图登录态失败")?;

        let mut sessions = Vec::with_capacity(rows.len());
        let mut legacy_sessions = Vec::new();
        for row in rows {
            let xingtu_account_id: String = row.try_get("xingtu_account_id")?;
            let encrypted_payload: Option<Vec<u8>> = row.try_get("encrypted_payload")?;
            let session = if let Some(encrypted_payload) = encrypted_payload {
                let encryption_key_id: Option<String> = row.try_get("encryption_key_id")?;
                if encryption_key_id.as_deref() != Some(self.cipher.key_id()) {
                    return Err(anyhow!(
                        "星图登录态密钥标识不匹配：account_id={xingtu_account_id} stored={} configured={}",
                        encryption_key_id.as_deref().unwrap_or("missing"),
                        self.cipher.key_id()
                    ));
                }
                self.cipher
                    .decrypt_session(&xingtu_account_id, &encrypted_payload)
                    .with_context(|| format!("解密星图登录态失败：{xingtu_account_id}"))?
            } else {
                let extra_headers_value: Value = row.try_get("extra_headers")?;
                let session = XingtuSession {
                    cookie: row
                        .try_get::<Option<String>, _>("cookie")?
                        .ok_or_else(|| anyhow!("历史星图登录态缺少 cookie：{xingtu_account_id}"))?,
                    csrf_token: row.try_get::<Option<String>, _>("csrf_token")?.ok_or_else(
                        || anyhow!("历史星图登录态缺少 csrf_token：{xingtu_account_id}"),
                    )?,
                    session_key: row.try_get("session_key")?,
                    user_agent: row.try_get("user_agent")?,
                    extra_headers: serde_json::from_value(extra_headers_value).unwrap_or_default(),
                };
                legacy_sessions.push((xingtu_account_id.clone(), session.clone()));
                session
            };
            sessions.push(StoredXingtuSession {
                xingtu_account_id,
                session,
            });
        }

        for (xingtu_account_id, session) in legacy_sessions {
            self.upsert_session(&xingtu_account_id, &session)
                .await
                .with_context(|| format!("迁移历史明文登录态失败：{xingtu_account_id}"))?;
        }

        Ok(sessions)
    }

    /// 查询需要巡检登录态的项目账号。
    pub async fn list_login_check_accounts(&self) -> anyhow::Result<Vec<XingtuProjectAccount>> {
        self.list_accounts_by_filter("login_check").await
    }

    /// 查询所有项目账号，供接口返回和调试。
    pub async fn list_accounts(&self) -> anyhow::Result<Vec<XingtuProjectAccount>> {
        self.list_accounts_by_filter("all").await
    }

    async fn list_accounts_by_filter(
        &self,
        filter: &str,
    ) -> anyhow::Result<Vec<XingtuProjectAccount>> {
        let rows = sqlx::query(
            r#"
            SELECT
                account.xingtu_account_id,
                account.project_id,
                project.project_key AS project,
                project.display_name AS project_display_name,
                account.display_name,
                project.notification_receive_id_type::text AS receive_id_type,
                project.notification_receive_id AS receive_id,
                account.ops_ids,
                project.login_notice_card_template_id,
                account.login_check_enabled,
                account.session_status::text AS session_status,
                account.last_checked_at,
                account.last_valid_at,
                account.last_invalid_at,
                account.last_error
            FROM xingtu_project_account account
            JOIN xingtu_project project ON project.project_id = account.project_id
            WHERE
                account.is_active = true
                AND project.is_active = true
                AND project.notification_receive_id IS NOT NULL
                AND (
                    $1 = 'all'
                    OR ($1 = 'login_check' AND account.login_check_enabled = true)
                )
            ORDER BY project.project_key, account.xingtu_account_id
            "#,
        )
        .bind(filter)
        .fetch_all(&self.pool)
        .await
        .context("查询星图项目账号失败")?;

        rows.into_iter()
            .map(row_to_project_account)
            .collect::<anyhow::Result<Vec<_>>>()
    }

    /// 更新登录态巡检状态。
    pub async fn update_session_check_status(
        &self,
        xingtu_account_id: &str,
        valid: bool,
        error_message: Option<&str>,
    ) -> anyhow::Result<()> {
        sqlx::query(
            r#"
            UPDATE xingtu_project_account
            SET
                session_status = CASE WHEN $2 THEN 'valid'::xingtu_session_status ELSE 'invalid'::xingtu_session_status END,
                last_checked_at = now(),
                last_valid_at = CASE WHEN $2 THEN now() ELSE last_valid_at END,
                last_invalid_at = CASE WHEN $2 THEN last_invalid_at ELSE now() END,
                last_error = $3
            WHERE xingtu_account_id = $1
            "#,
        )
        .bind(xingtu_account_id)
        .bind(valid)
        .bind(error_message)
        .execute(&self.pool)
        .await
        .with_context(|| format!("更新星图登录态检查状态失败：{xingtu_account_id}"))?;

        Ok(())
    }
}

/// 内存中的星图客户端注册表。
///
/// 服务启动后可以先为空；外部通过登录态接收接口写入后，
/// 调度器会按 `xingtu_account_id` 取出对应客户端执行星图请求。
#[derive(Clone, Default)]
pub struct XingtuSessionRegistry {
    clients: Arc<RwLock<HashMap<String, crate::xingtu::XingtuClient>>>,
}

impl XingtuSessionRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// 写入或替换某个项目账号的星图客户端登录态。
    pub async fn set_session(
        &self,
        xingtu_account_id: &str,
        session: XingtuSession,
    ) -> anyhow::Result<()> {
        if xingtu_account_id.trim().is_empty() {
            return Err(anyhow!("xingtu_account_id 不能为空"));
        }

        let client = crate::xingtu::XingtuClient::new()?;
        client.set_session(session).await?;
        self.clients
            .write()
            .await
            .insert(xingtu_account_id.trim().to_string(), client);
        Ok(())
    }

    /// 获取某个项目账号的星图客户端。
    pub async fn get_client(
        &self,
        xingtu_account_id: &str,
    ) -> anyhow::Result<crate::xingtu::XingtuClient> {
        self.clients
            .read()
            .await
            .get(xingtu_account_id.trim())
            .cloned()
            .ok_or_else(|| anyhow!("星图账号 `{xingtu_account_id}` 未注入登录态"))
    }

    /// 当前内存中已有登录态的账号 ID。
    pub async fn account_ids(&self) -> Vec<String> {
        self.clients.read().await.keys().cloned().collect()
    }
}

fn row_to_project_account(row: sqlx::postgres::PgRow) -> anyhow::Result<XingtuProjectAccount> {
    Ok(XingtuProjectAccount {
        xingtu_account_id: row.try_get("xingtu_account_id")?,
        project_id: row.try_get("project_id")?,
        project: row.try_get("project")?,
        project_display_name: row.try_get("project_display_name")?,
        display_name: row.try_get("display_name")?,
        receive_id_type: row.try_get("receive_id_type")?,
        receive_id: row.try_get("receive_id")?,
        ops_ids: row.try_get("ops_ids")?,
        login_notice_card_template_id: row.try_get("login_notice_card_template_id")?,
        login_check_enabled: row.try_get("login_check_enabled")?,
        session_status: row.try_get("session_status")?,
        last_checked_at: row.try_get("last_checked_at")?,
        last_valid_at: row.try_get("last_valid_at")?,
        last_invalid_at: row.try_get("last_invalid_at")?,
        last_error: row.try_get("last_error")?,
    })
}
