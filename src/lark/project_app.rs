use crate::client::LarkClient;
use crate::config::Config;
use crate::server::secret_store::ProjectFeishuCredentialCipher;
use anyhow::{Context, anyhow};
use chrono::{DateTime, Utc};
use sqlx::{PgPool, Postgres, Row, Transaction};
use std::{collections::HashMap, sync::Arc};
use tokio::sync::RwLock;

#[derive(Debug, Clone)]
pub struct FeishuAppMetadata {
    pub feishu_app_id: i64,
    pub app_id: String,
    pub display_name: String,
    pub encryption_key_id: String,
    pub is_active: bool,
    pub project_count: i64,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone)]
pub struct ProjectFeishuAppBinding {
    pub project_id: i64,
    pub app: FeishuAppMetadata,
    pub bound_at: DateTime<Utc>,
    pub binding_updated_at: DateTime<Utc>,
}

#[derive(Clone)]
struct CachedProjectClient {
    app_id: String,
    updated_at: DateTime<Utc>,
    client: LarkClient,
}

/// 管理飞书开放平台应用及项目绑定，并按项目解析独立客户端。
#[derive(Clone)]
pub struct ProjectFeishuAppStore {
    pool: PgPool,
    cipher: ProjectFeishuCredentialCipher,
    fallback_client: LarkClient,
    fallback_base_url: String,
    clients: Arc<RwLock<HashMap<i64, CachedProjectClient>>>,
}

impl ProjectFeishuAppStore {
    pub fn new(
        pool: PgPool,
        cipher: ProjectFeishuCredentialCipher,
        fallback_config: Config,
        fallback_client: LarkClient,
    ) -> Self {
        Self {
            pool,
            cipher,
            fallback_client,
            fallback_base_url: fallback_config.lark_base_url,
            clients: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// 项目未绑定应用时使用全局环境变量客户端，保证既有项目平滑升级。
    pub async fn client_for_project(&self, project_id: i64) -> anyhow::Result<LarkClient> {
        let row = sqlx::query(
            r#"
            SELECT app.feishu_app_id, app.app_id, app.encrypted_app_secret,
                app.encryption_key_id, app.is_active, app.updated_at
            FROM xingtu_project_feishu_app_binding binding
            JOIN xingtu_feishu_app app ON app.feishu_app_id = binding.feishu_app_id
            WHERE binding.project_id = $1
            "#,
        )
        .bind(project_id)
        .fetch_optional(&self.pool)
        .await
        .with_context(|| format!("读取项目 {project_id} 的飞书应用绑定失败"))?;

        let Some(row) = row else {
            return Ok(self.fallback_client.clone());
        };
        let feishu_app_id: i64 = row.try_get("feishu_app_id")?;
        let app_id: String = row.try_get("app_id")?;
        let encrypted_app_secret: Vec<u8> = row.try_get("encrypted_app_secret")?;
        let encryption_key_id: String = row.try_get("encryption_key_id")?;
        let is_active: bool = row.try_get("is_active")?;
        let updated_at: DateTime<Utc> = row.try_get("updated_at")?;
        if !is_active {
            return Err(anyhow!("项目 {project_id} 绑定的飞书应用已停用"));
        }
        if encryption_key_id != self.cipher.key_id() {
            return Err(anyhow!(
                "飞书应用 {feishu_app_id} 使用密钥 `{encryption_key_id}` 加密，当前密钥 ID 为 `{}`，请使用原密钥重新加密后再启动",
                self.cipher.key_id()
            ));
        }
        if let Some(cached) = self.clients.read().await.get(&feishu_app_id)
            && cached.app_id == app_id
            && cached.updated_at == updated_at
        {
            return Ok(cached.client.clone());
        }

        let app_secret = self
            .cipher
            .decrypt_app_secret(feishu_app_id, &app_id, &encrypted_app_secret)
            .with_context(|| format!("解密飞书应用 {feishu_app_id} 的 APP_SECRET 失败"))?;
        let client = LarkClient::new(Config {
            lark_app_id: app_id.clone(),
            lark_app_secret: app_secret,
            lark_base_url: self.fallback_base_url.clone(),
        })
        .with_context(|| format!("创建飞书应用 {feishu_app_id} 的客户端失败"))?;
        self.clients.write().await.insert(
            feishu_app_id,
            CachedProjectClient {
                app_id,
                updated_at,
                client: client.clone(),
            },
        );
        Ok(client)
    }

    pub async fn list(&self, include_inactive: bool) -> anyhow::Result<Vec<FeishuAppMetadata>> {
        let rows = sqlx::query(feishu_app_select_sql())
            .bind(include_inactive)
            .fetch_all(&self.pool)
            .await?;
        rows.into_iter().map(feishu_app_from_row).collect()
    }

    pub async fn get(&self, feishu_app_id: i64) -> anyhow::Result<Option<FeishuAppMetadata>> {
        let row = sqlx::query(&format!(
            "{} AND app.feishu_app_id = $2",
            feishu_app_select_sql()
        ))
        .bind(true)
        .bind(feishu_app_id)
        .fetch_optional(&self.pool)
        .await?;
        row.map(feishu_app_from_row).transpose()
    }

    pub async fn create(
        &self,
        app_id: &str,
        app_secret: &str,
        display_name: &str,
    ) -> anyhow::Result<FeishuAppMetadata> {
        let (app_id, app_secret, display_name) = validate_input(app_id, app_secret, display_name)?;
        let mut tx = self.pool.begin().await?;
        let feishu_app_id: i64 =
            sqlx::query_scalar("SELECT nextval('xingtu_feishu_app_feishu_app_id_seq')")
                .fetch_one(&mut *tx)
                .await?;
        let encrypted = self
            .cipher
            .encrypt_app_secret(feishu_app_id, app_id, app_secret)?;
        sqlx::query(
            r#"
            INSERT INTO xingtu_feishu_app (
                feishu_app_id, app_id, display_name, encrypted_app_secret, encryption_key_id
            ) VALUES ($1, $2, $3, $4, $5)
            "#,
        )
        .bind(feishu_app_id)
        .bind(app_id)
        .bind(display_name)
        .bind(encrypted)
        .bind(self.cipher.key_id())
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        self.get(feishu_app_id)
            .await?
            .ok_or_else(|| anyhow!("飞书应用创建后未能读取"))
    }

    pub async fn replace(
        &self,
        feishu_app_id: i64,
        app_id: &str,
        app_secret: &str,
        display_name: &str,
        is_active: bool,
    ) -> anyhow::Result<Option<FeishuAppMetadata>> {
        let (app_id, app_secret, display_name) = validate_input(app_id, app_secret, display_name)?;
        let encrypted = self
            .cipher
            .encrypt_app_secret(feishu_app_id, app_id, app_secret)?;
        let result = sqlx::query(
            r#"
            UPDATE xingtu_feishu_app SET app_id = $2, display_name = $3,
                encrypted_app_secret = $4, encryption_key_id = $5,
                is_active = $6, updated_at = now()
            WHERE feishu_app_id = $1
            "#,
        )
        .bind(feishu_app_id)
        .bind(app_id)
        .bind(display_name)
        .bind(encrypted)
        .bind(self.cipher.key_id())
        .bind(is_active)
        .execute(&self.pool)
        .await?;
        self.clients.write().await.remove(&feishu_app_id);
        if result.rows_affected() == 0 {
            return Ok(None);
        }
        self.get(feishu_app_id).await
    }

    pub async fn bind_project(
        &self,
        project_id: i64,
        feishu_app_id: i64,
    ) -> anyhow::Result<ProjectFeishuAppBinding> {
        let mut tx = self.pool.begin().await?;
        lock_active_app(&mut tx, feishu_app_id).await?;
        sqlx::query(
            r#"
            INSERT INTO xingtu_project_feishu_app_binding (project_id, feishu_app_id)
            VALUES ($1, $2)
            ON CONFLICT (project_id) DO UPDATE SET
                feishu_app_id = EXCLUDED.feishu_app_id, updated_at = now()
            "#,
        )
        .bind(project_id)
        .bind(feishu_app_id)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        self.get_project_binding(project_id)
            .await?
            .ok_or_else(|| anyhow!("项目飞书应用绑定后未能读取"))
    }

    pub async fn get_project_binding(
        &self,
        project_id: i64,
    ) -> anyhow::Result<Option<ProjectFeishuAppBinding>> {
        let row = sqlx::query(
            r#"
            SELECT binding.project_id, binding.created_at AS bound_at,
                binding.updated_at AS binding_updated_at,
                app.feishu_app_id, app.app_id, app.display_name, app.encryption_key_id,
                app.is_active, app.created_at, app.updated_at,
                (SELECT count(*) FROM xingtu_project_feishu_app_binding item
                 WHERE item.feishu_app_id = app.feishu_app_id) AS project_count
            FROM xingtu_project_feishu_app_binding binding
            JOIN xingtu_feishu_app app ON app.feishu_app_id = binding.feishu_app_id
            WHERE binding.project_id = $1
            "#,
        )
        .bind(project_id)
        .fetch_optional(&self.pool)
        .await?;
        row.map(|row| {
            Ok(ProjectFeishuAppBinding {
                project_id: row.try_get("project_id")?,
                bound_at: row.try_get("bound_at")?,
                binding_updated_at: row.try_get("binding_updated_at")?,
                app: feishu_app_from_row(row)?,
            })
        })
        .transpose()
    }

    pub async fn unbind_project(&self, project_id: i64) -> anyhow::Result<bool> {
        let result =
            sqlx::query("DELETE FROM xingtu_project_feishu_app_binding WHERE project_id = $1")
                .bind(project_id)
                .execute(&self.pool)
                .await?;
        Ok(result.rows_affected() > 0)
    }
}

fn validate_input<'a>(
    app_id: &'a str,
    app_secret: &'a str,
    display_name: &'a str,
) -> anyhow::Result<(&'a str, &'a str, &'a str)> {
    let (app_id, app_secret, display_name) =
        (app_id.trim(), app_secret.trim(), display_name.trim());
    if app_id.is_empty() || app_secret.is_empty() || display_name.is_empty() {
        return Err(anyhow!("app_id、app_secret 和 display_name 均不能为空"));
    }
    Ok((app_id, app_secret, display_name))
}

async fn lock_active_app(
    tx: &mut Transaction<'_, Postgres>,
    feishu_app_id: i64,
) -> anyhow::Result<()> {
    let active = sqlx::query_scalar::<_, bool>(
        "SELECT is_active FROM xingtu_feishu_app WHERE feishu_app_id = $1 FOR SHARE",
    )
    .bind(feishu_app_id)
    .fetch_optional(&mut **tx)
    .await?
    .ok_or_else(|| anyhow!("飞书应用不存在：{feishu_app_id}"))?;
    if !active {
        return Err(anyhow!("飞书应用已停用：{feishu_app_id}"));
    }
    Ok(())
}

fn feishu_app_select_sql() -> &'static str {
    r#"
    SELECT app.feishu_app_id, app.app_id, app.display_name, app.encryption_key_id,
        app.is_active, app.created_at, app.updated_at,
        (SELECT count(*) FROM xingtu_project_feishu_app_binding binding
         WHERE binding.feishu_app_id = app.feishu_app_id) AS project_count
    FROM xingtu_feishu_app app
    WHERE ($1::boolean OR app.is_active = true)
    "#
}

fn feishu_app_from_row(row: sqlx::postgres::PgRow) -> anyhow::Result<FeishuAppMetadata> {
    Ok(FeishuAppMetadata {
        feishu_app_id: row.try_get("feishu_app_id")?,
        app_id: row.try_get("app_id")?,
        display_name: row.try_get("display_name")?,
        encryption_key_id: row.try_get("encryption_key_id")?,
        is_active: row.try_get("is_active")?,
        project_count: row.try_get("project_count")?,
        created_at: row.try_get("created_at")?,
        updated_at: row.try_get("updated_at")?,
    })
}
