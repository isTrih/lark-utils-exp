use chrono::{DateTime, Utc};
use salvo::oapi::ToSchema;
use serde::Serialize;
use serde_json::Value;
use sha1::{Digest, Sha1};
use sqlx::{PgPool, Postgres, Row, Transaction};
use std::time::Duration;

const WORKFLOW_ADVISORY_LOCK_NAME: &str = "lark-utils-exp:workflow-write-lock:v1";

#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct WorkflowRunRecord {
    pub workflow_run_id: i64,
    pub workflow_kind: String,
    pub scope_activity_period_id: Option<i64>,
    pub trigger_source: String,
    pub status: String,
    pub started_at: DateTime<Utc>,
    pub finished_at: Option<DateTime<Utc>>,
    pub summary: Value,
    pub error_code: Option<String>,
    pub error_message: Option<String>,
    pub request_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct WorkflowStepRecord {
    pub workflow_step_id: i64,
    pub workflow_run_id: i64,
    pub activity_period_id: Option<i64>,
    pub step_name: String,
    pub status: String,
    pub idempotency_key: Option<String>,
    pub started_at: DateTime<Utc>,
    pub finished_at: Option<DateTime<Utc>>,
    pub summary: Value,
    pub error_message: Option<String>,
}

#[derive(Debug, Clone)]
pub struct WorkflowRunRepository {
    pool: PgPool,
}

impl WorkflowRunRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    pub async fn start_run(
        &self,
        kind: &str,
        scope_activity_period_id: Option<i64>,
        trigger_source: &str,
        request_id: Option<&str>,
    ) -> anyhow::Result<i64> {
        let row = sqlx::query(
            r#"
            INSERT INTO workflow_run (
                workflow_kind, scope_activity_period_id, trigger_source, request_id
            )
            VALUES ($1, $2, $3, $4)
            RETURNING workflow_run_id
            "#,
        )
        .bind(kind)
        .bind(scope_activity_period_id)
        .bind(trigger_source)
        .bind(request_id)
        .fetch_one(&self.pool)
        .await?;
        Ok(row.try_get("workflow_run_id")?)
    }

    /// 事务级 advisory lock 会在 commit/rollback 或连接异常时自动释放。
    pub async fn try_acquire_global_lock(
        &self,
    ) -> anyhow::Result<Option<Transaction<'static, Postgres>>> {
        let mut tx = self.pool.begin().await?;
        let acquired: bool =
            sqlx::query_scalar("SELECT pg_try_advisory_xact_lock(hashtextextended($1, 0))")
                .bind(WORKFLOW_ADVISORY_LOCK_NAME)
                .fetch_one(&mut *tx)
                .await?;
        if acquired {
            Ok(Some(tx))
        } else {
            tx.rollback().await?;
            Ok(None)
        }
    }

    pub async fn finish_run_success(&self, run_id: i64, summary: Value) -> anyhow::Result<()> {
        sqlx::query(
            "UPDATE workflow_run SET status = 'succeeded', finished_at = now(), summary = $2 WHERE workflow_run_id = $1",
        )
        .bind(run_id)
        .bind(summary)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn finish_run_failure(
        &self,
        run_id: i64,
        status: &str,
        code: &str,
        error: &str,
    ) -> anyhow::Result<()> {
        sqlx::query(
            "UPDATE workflow_run SET status = $2, finished_at = now(), error_code = $3, error_message = $4 WHERE workflow_run_id = $1",
        )
        .bind(run_id)
        .bind(status)
        .bind(code)
        .bind(truncate_error(error))
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn start_step(
        &self,
        run_id: i64,
        activity_period_id: Option<i64>,
        step_name: &str,
        idempotency_key: Option<&str>,
    ) -> anyhow::Result<i64> {
        let row = sqlx::query(
            r#"
            INSERT INTO workflow_step (
                workflow_run_id, activity_period_id, step_name, idempotency_key
            )
            VALUES ($1, $2, $3, $4)
            ON CONFLICT (workflow_run_id, activity_period_id, step_name)
            DO UPDATE SET status = 'running', started_at = now(), finished_at = NULL,
                summary = '{}'::jsonb, error_message = NULL
            RETURNING workflow_step_id
            "#,
        )
        .bind(run_id)
        .bind(activity_period_id)
        .bind(step_name)
        .bind(idempotency_key)
        .fetch_one(&self.pool)
        .await?;
        Ok(row.try_get("workflow_step_id")?)
    }

    pub async fn finish_step_success(&self, step_id: i64, summary: Value) -> anyhow::Result<()> {
        sqlx::query(
            "UPDATE workflow_step SET status = 'succeeded', finished_at = now(), summary = $2 WHERE workflow_step_id = $1",
        )
        .bind(step_id)
        .bind(summary)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn finish_step_failure(&self, step_id: i64, error: &str) -> anyhow::Result<()> {
        sqlx::query(
            "UPDATE workflow_step SET status = 'failed', finished_at = now(), error_message = $2 WHERE workflow_step_id = $1",
        )
        .bind(step_id)
        .bind(truncate_error(error))
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn list_runs(
        &self,
        limit: i64,
        offset: i64,
    ) -> anyhow::Result<Vec<WorkflowRunRecord>> {
        let rows = sqlx::query(
            r#"
            SELECT workflow_run_id, workflow_kind, scope_activity_period_id, trigger_source,
                status, started_at, finished_at, summary, error_code, error_message, request_id
            FROM workflow_run
            ORDER BY started_at DESC, workflow_run_id DESC
            LIMIT $1 OFFSET $2
            "#,
        )
        .bind(limit.clamp(1, 500))
        .bind(offset.max(0))
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter().map(run_from_row).collect()
    }

    pub async fn list_steps(&self, run_id: i64) -> anyhow::Result<Vec<WorkflowStepRecord>> {
        let rows = sqlx::query(
            r#"
            SELECT workflow_step_id, workflow_run_id, activity_period_id, step_name, status,
                idempotency_key, started_at, finished_at, summary, error_message
            FROM workflow_step
            WHERE workflow_run_id = $1
            ORDER BY started_at, workflow_step_id
            "#,
        )
        .bind(run_id)
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter().map(step_from_row).collect()
    }

    pub async fn claim_notification(
        &self,
        category: &str,
        scope_key: &str,
        payload: &str,
        cooldown: Duration,
    ) -> anyhow::Result<bool> {
        let payload_hash = sha1_hex(payload);
        let cooldown_seconds = i32::try_from(cooldown.as_secs()).unwrap_or(i32::MAX);
        let claimed = sqlx::query(
            r#"
            INSERT INTO notification_delivery_guard (
                category, scope_key, payload_hash, reserved_until
            )
            VALUES ($1, $2, $3, now() + interval '5 minutes')
            ON CONFLICT (category, scope_key)
            DO UPDATE SET
                last_sent_at = CASE
                    WHEN notification_delivery_guard.payload_hash IS DISTINCT FROM EXCLUDED.payload_hash
                    THEN NULL
                    ELSE notification_delivery_guard.last_sent_at
                END,
                payload_hash = EXCLUDED.payload_hash,
                reserved_until = EXCLUDED.reserved_until,
                updated_at = now()
            WHERE
                notification_delivery_guard.reserved_until <= now()
                AND (
                    notification_delivery_guard.payload_hash IS DISTINCT FROM EXCLUDED.payload_hash
                    OR notification_delivery_guard.last_sent_at IS NULL
                    OR notification_delivery_guard.last_sent_at <= now() - make_interval(secs => $4)
                )
            RETURNING 1
            "#,
        )
        .bind(category)
        .bind(scope_key)
        .bind(payload_hash)
        .bind(cooldown_seconds)
        .fetch_optional(&self.pool)
        .await?;
        Ok(claimed.is_some())
    }

    pub async fn mark_notification_sent(
        &self,
        category: &str,
        scope_key: &str,
    ) -> anyhow::Result<()> {
        sqlx::query(
            "UPDATE notification_delivery_guard SET last_sent_at = now(), reserved_until = now(), updated_at = now() WHERE category = $1 AND scope_key = $2",
        )
        .bind(category)
        .bind(scope_key)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn release_notification(
        &self,
        category: &str,
        scope_key: &str,
    ) -> anyhow::Result<()> {
        sqlx::query(
            "UPDATE notification_delivery_guard SET reserved_until = now(), updated_at = now() WHERE category = $1 AND scope_key = $2",
        )
        .bind(category)
        .bind(scope_key)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn clear_notification(&self, category: &str, scope_key: &str) -> anyhow::Result<()> {
        sqlx::query(
            "DELETE FROM notification_delivery_guard WHERE category = $1 AND scope_key = $2",
        )
        .bind(category)
        .bind(scope_key)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// 服务异常退出会遗留 running 台账；只回收远超正常执行窗口的记录，避免误伤其他实例。
    pub async fn reconcile_stale_runs(&self, max_age: Duration) -> anyhow::Result<u64> {
        let seconds = i32::try_from(max_age.as_secs()).unwrap_or(i32::MAX);
        let mut tx = self.pool.begin().await?;
        sqlx::query(
            r#"
            UPDATE workflow_step
            SET status = 'failed', finished_at = now(),
                error_message = COALESCE(error_message, '服务异常退出，阶段台账由启动恢复标记失败')
            WHERE status = 'running'
                AND started_at < now() - make_interval(secs => $1)
            "#,
        )
        .bind(seconds)
        .execute(&mut *tx)
        .await?;
        let result = sqlx::query(
            r#"
            UPDATE workflow_run
            SET status = 'failed', finished_at = now(), error_code = 'stale_run',
                error_message = COALESCE(error_message, '服务异常退出，运行台账由启动恢复标记失败')
            WHERE status = 'running'
                AND started_at < now() - make_interval(secs => $1)
            "#,
        )
        .bind(seconds)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(result.rows_affected())
    }
}

fn truncate_error(error: &str) -> String {
    error.chars().take(4_000).collect()
}

fn sha1_hex(value: &str) -> String {
    let mut digest = Sha1::new();
    digest.update(value.as_bytes());
    format!("{:x}", digest.finalize())
}

fn run_from_row(row: sqlx::postgres::PgRow) -> anyhow::Result<WorkflowRunRecord> {
    Ok(WorkflowRunRecord {
        workflow_run_id: row.try_get("workflow_run_id")?,
        workflow_kind: row.try_get("workflow_kind")?,
        scope_activity_period_id: row.try_get("scope_activity_period_id")?,
        trigger_source: row.try_get("trigger_source")?,
        status: row.try_get("status")?,
        started_at: row.try_get("started_at")?,
        finished_at: row.try_get("finished_at")?,
        summary: row.try_get("summary")?,
        error_code: row.try_get("error_code")?,
        error_message: row.try_get("error_message")?,
        request_id: row.try_get("request_id")?,
    })
}

fn step_from_row(row: sqlx::postgres::PgRow) -> anyhow::Result<WorkflowStepRecord> {
    Ok(WorkflowStepRecord {
        workflow_step_id: row.try_get("workflow_step_id")?,
        workflow_run_id: row.try_get("workflow_run_id")?,
        activity_period_id: row.try_get("activity_period_id")?,
        step_name: row.try_get("step_name")?,
        status: row.try_get("status")?,
        idempotency_key: row.try_get("idempotency_key")?,
        started_at: row.try_get("started_at")?,
        finished_at: row.try_get("finished_at")?,
        summary: row.try_get("summary")?,
        error_message: row.try_get("error_message")?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_text_is_bounded_and_notification_hash_is_stable() {
        assert_eq!(truncate_error(&"错误".repeat(5_000)).chars().count(), 4_000);
        assert_eq!(sha1_hex("same"), sha1_hex("same"));
        assert_ne!(sha1_hex("same"), sha1_hex("changed"));
    }
}
