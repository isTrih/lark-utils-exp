use crate::lark::im::MessageReceiver;
use anyhow::{Context, anyhow};
use chrono::{DateTime, NaiveDate, Utc};
use salvo::oapi::ToSchema;
use serde::{Deserialize, Serialize};
use sqlx::postgres::PgRow;
use sqlx::{PgPool, Row};

/// 可持久化的卡片消息类别。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum CardMessageCategory {
    /// 工作流和星图相关错误通知。
    ErrorLog,
    /// 待审核数量通知。
    Audit,
    /// 项目日报卡片。
    DailyReport,
}

impl CardMessageCategory {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ErrorLog => "error_log",
            Self::Audit => "audit",
            Self::DailyReport => "daily_report",
        }
    }

    pub fn parse(value: &str) -> anyhow::Result<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "error_log" => Ok(Self::ErrorLog),
            "audit" => Ok(Self::Audit),
            "daily_report" => Ok(Self::DailyReport),
            other => Err(anyhow!(
                "不支持的消息类别 `{other}`，可选值：error_log / audit / daily_report"
            )),
        }
    }
}

/// 卡片消息发送及撤回历史。
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct CardMessageHistory {
    pub card_message_history_id: i64,
    pub message_id: String,
    pub category: CardMessageCategory,
    pub summary: String,
    pub receive_id_type: String,
    pub receive_id: String,
    pub project_name: Option<String>,
    pub activity_period_id: Option<i64>,
    pub sent_at: DateTime<Utc>,
    pub last_recall_attempt_at: Option<DateTime<Utc>>,
    pub recalled_at: Option<DateTime<Utc>>,
    pub recall_error: Option<String>,
}

/// 卡片消息历史查询条件。
#[derive(Debug, Clone, Copy)]
pub struct CardMessageHistoryFilter {
    pub category: Option<CardMessageCategory>,
    pub date_from: Option<NaiveDate>,
    pub date_to: Option<NaiveDate>,
    pub limit: i64,
    pub offset: i64,
}

/// 卡片消息历史仓储。
#[derive(Debug, Clone)]
pub struct CardMessageHistoryRepository {
    pool: PgPool,
}

impl CardMessageHistoryRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// 记录一次成功发送。历史记录失败不应触发业务重试，避免重复发卡。
    pub async fn record_sent_message(
        &self,
        message_id: Option<&str>,
        category: CardMessageCategory,
        summary: impl Into<String>,
        receiver: &MessageReceiver,
        project_name: Option<&str>,
        activity_period_id: Option<i64>,
    ) {
        let Some(message_id) = message_id.map(str::trim).filter(|value| !value.is_empty()) else {
            tracing::error!(
                category = category.as_str(),
                receive_id = %receiver.receive_id,
                "飞书卡片发送成功但响应中缺少 message_id，无法记录发送历史"
            );
            return;
        };
        let summary = summary.into();

        if let Err(error) = self
            .insert_sent(
                message_id,
                category,
                &summary,
                receiver,
                project_name,
                activity_period_id,
            )
            .await
        {
            tracing::error!(
                message_id,
                category = category.as_str(),
                error = ?error,
                "记录飞书卡片消息发送历史失败；消息已经发送，不执行重试"
            );
        }
    }

    async fn insert_sent(
        &self,
        message_id: &str,
        category: CardMessageCategory,
        summary: &str,
        receiver: &MessageReceiver,
        project_name: Option<&str>,
        activity_period_id: Option<i64>,
    ) -> anyhow::Result<()> {
        if summary.trim().is_empty() {
            return Err(anyhow!("卡片消息摘要不能为空"));
        }
        if receiver.receive_id.trim().is_empty() {
            return Err(anyhow!("卡片消息接收者 ID 不能为空"));
        }

        sqlx::query(
            r#"
            INSERT INTO card_message_history (
                message_id,
                category,
                summary,
                receive_id_type,
                receive_id,
                project_name,
                activity_period_id,
                sent_at
            )
            VALUES ($1, $2, $3, $4::xingtu_receive_id_type, $5, $6, $7, now())
            ON CONFLICT (message_id) DO NOTHING
            "#,
        )
        .bind(message_id)
        .bind(category.as_str())
        .bind(summary.trim())
        .bind(receiver.receive_id_type.as_str())
        .bind(receiver.receive_id.trim())
        .bind(trim_optional(project_name))
        .bind(activity_period_id)
        .execute(&self.pool)
        .await
        .with_context(|| format!("写入卡片消息历史失败：message_id={message_id}"))?;

        Ok(())
    }

    pub async fn list(
        &self,
        filter: CardMessageHistoryFilter,
    ) -> anyhow::Result<Vec<CardMessageHistory>> {
        let rows = sqlx::query(
            r#"
            SELECT
                card_message_history_id,
                message_id,
                category,
                summary,
                receive_id_type::text AS receive_id_type,
                receive_id,
                project_name,
                activity_period_id,
                sent_at,
                last_recall_attempt_at,
                recalled_at,
                recall_error
            FROM card_message_history
            WHERE
                ($1::text IS NULL OR category = $1)
                AND (
                    $2::date IS NULL
                    OR sent_at >= (($2::date)::timestamp AT TIME ZONE 'Asia/Shanghai')
                )
                AND (
                    $3::date IS NULL
                    OR sent_at < ((($3::date + 1)::timestamp) AT TIME ZONE 'Asia/Shanghai')
                )
            ORDER BY sent_at DESC, card_message_history_id DESC
            LIMIT $4 OFFSET $5
            "#,
        )
        .bind(filter.category.map(CardMessageCategory::as_str))
        .bind(filter.date_from)
        .bind(filter.date_to)
        .bind(filter.limit)
        .bind(filter.offset)
        .fetch_all(&self.pool)
        .await
        .context("查询卡片消息发送历史失败")?;

        rows.into_iter().map(history_from_row).collect()
    }

    pub async fn find_by_message_id(
        &self,
        message_id: &str,
    ) -> anyhow::Result<Option<CardMessageHistory>> {
        let row = sqlx::query(
            r#"
            SELECT
                card_message_history_id,
                message_id,
                category,
                summary,
                receive_id_type::text AS receive_id_type,
                receive_id,
                project_name,
                activity_period_id,
                sent_at,
                last_recall_attempt_at,
                recalled_at,
                recall_error
            FROM card_message_history
            WHERE message_id = $1
            "#,
        )
        .bind(message_id)
        .fetch_optional(&self.pool)
        .await
        .with_context(|| format!("查询卡片消息历史失败：message_id={message_id}"))?;

        row.map(history_from_row).transpose()
    }

    pub async fn mark_recall_started(&self, message_id: &str) -> anyhow::Result<()> {
        sqlx::query(
            r#"
            UPDATE card_message_history
            SET
                last_recall_attempt_at = now(),
                recall_error = NULL
            WHERE message_id = $1
            "#,
        )
        .bind(message_id)
        .execute(&self.pool)
        .await
        .with_context(|| format!("记录消息撤回尝试失败：message_id={message_id}"))?;
        Ok(())
    }

    pub async fn mark_recalled(&self, message_id: &str) -> anyhow::Result<CardMessageHistory> {
        let row = sqlx::query(
            r#"
            UPDATE card_message_history
            SET
                last_recall_attempt_at = now(),
                recalled_at = COALESCE(recalled_at, now()),
                recall_error = NULL
            WHERE message_id = $1
            RETURNING
                card_message_history_id,
                message_id,
                category,
                summary,
                receive_id_type::text AS receive_id_type,
                receive_id,
                project_name,
                activity_period_id,
                sent_at,
                last_recall_attempt_at,
                recalled_at,
                recall_error
            "#,
        )
        .bind(message_id)
        .fetch_one(&self.pool)
        .await
        .with_context(|| format!("记录消息撤回成功状态失败：message_id={message_id}"))?;

        history_from_row(row)
    }

    pub async fn mark_recall_failed(&self, message_id: &str, error: &str) -> anyhow::Result<()> {
        sqlx::query(
            r#"
            UPDATE card_message_history
            SET
                last_recall_attempt_at = now(),
                recall_error = $2
            WHERE message_id = $1
            "#,
        )
        .bind(message_id)
        .bind(error)
        .execute(&self.pool)
        .await
        .with_context(|| format!("记录消息撤回失败状态失败：message_id={message_id}"))?;
        Ok(())
    }
}

fn history_from_row(row: PgRow) -> anyhow::Result<CardMessageHistory> {
    let category: String = row.try_get("category")?;
    Ok(CardMessageHistory {
        card_message_history_id: row.try_get("card_message_history_id")?,
        message_id: row.try_get("message_id")?,
        category: CardMessageCategory::parse(&category)?,
        summary: row.try_get("summary")?,
        receive_id_type: row.try_get("receive_id_type")?,
        receive_id: row.try_get("receive_id")?,
        project_name: row.try_get("project_name")?,
        activity_period_id: row.try_get("activity_period_id")?,
        sent_at: row.try_get("sent_at")?,
        last_recall_attempt_at: row.try_get("last_recall_attempt_at")?,
        recalled_at: row.try_get("recalled_at")?,
        recall_error: row.try_get("recall_error")?,
    })
}

fn trim_optional(value: Option<&str>) -> Option<&str> {
    value.map(str::trim).filter(|value| !value.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn card_message_categories_use_stable_api_values() {
        assert_eq!(CardMessageCategory::ErrorLog.as_str(), "error_log");
        assert_eq!(CardMessageCategory::Audit.as_str(), "audit");
        assert_eq!(CardMessageCategory::DailyReport.as_str(), "daily_report");
        assert_eq!(
            CardMessageCategory::parse(" DAILY_REPORT ").unwrap(),
            CardMessageCategory::DailyReport
        );
        assert!(CardMessageCategory::parse("unknown").is_err());
    }
}
