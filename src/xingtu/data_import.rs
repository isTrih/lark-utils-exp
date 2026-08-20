use crate::lark::sheets::get_spreadsheet_token;
use crate::pipeline::sheet::read_first_sheet_rows;
use crate::pipeline::sync::{FieldValueRules, SourceRow, parse_sheet_rows, value_to_plain_string};
use crate::pipeline::video::VIDEO_UNIQUE_KEY_FIELD;
use crate::{client::LarkClient, pipeline::live::LIVE_UNIQUE_KEY_FIELD};
use anyhow::{Context, anyhow};
use chrono::{NaiveDate, NaiveDateTime, TimeZone};
use chrono_tz::Asia::Shanghai;
use salvo::oapi::ToSchema;
use serde::Serialize;
use serde_json::{Map, Value};
use sqlx::types::Json;
use sqlx::{PgPool, Postgres, Row, Transaction};

const XINGTU_DATA_SOURCE: &str = "星图数据";
const DEFAULT_DATA_SOURCE_FIELD: &str = "数据来源";
const DEFAULT_MANUAL_DATA_SOURCE: &str = "手动登记";
const DEFAULT_MANUAL_AUTO_APPROVE_RESULT: &str = "审核通过";
const AUDIT_RESULT_UPDATE_BATCH_SIZE: usize = 1000;

/// 从审核表解析出的单条数据库回写数据。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuditResultUpdate {
    pub unique_key: String,
    pub audit_result: String,
    pub label: Option<String>,
    pub audit_extra: Value,
}

/// 合并后写入数据库的一行。
#[derive(Debug, Clone)]
pub struct MergedRecordForDb {
    pub unique_key: String,
    pub data_source: String,
    pub fields: Map<String, Value>,
}

impl From<SourceRow> for MergedRecordForDb {
    fn from(row: SourceRow) -> Self {
        Self::from_source_row(row, DEFAULT_DATA_SOURCE_FIELD, XINGTU_DATA_SOURCE)
    }
}

impl MergedRecordForDb {
    /// 从同步 pipeline 的 SourceRow 转成数据库入库行。
    ///
    /// `data_source` 只用于判断手动登记是否自动审核通过，不会直接写入数据库。
    pub fn from_source_row(
        row: SourceRow,
        data_source_field: &str,
        default_data_source: &str,
    ) -> Self {
        let data_source = row
            .fields
            .get(data_source_field)
            .and_then(value_to_plain_string)
            .unwrap_or_else(|| default_data_source.to_string());

        Self {
            unique_key: row.unique_key,
            data_source,
            fields: row.fields,
        }
    }
}

/// 合并数据入库时需要的运行时上下文。
#[derive(Debug, Clone)]
pub struct PersistMergedRecordsOptions {
    pub content_config_id: i64,
    pub content_type: String,
    pub feishu_source_id: Option<i64>,
    pub stat_date: NaiveDate,
    pub is_daily_final: bool,
    pub trace_enabled: bool,
    pub manual_source_value: String,
    pub manual_auto_approve: bool,
    pub manual_auto_approve_result: String,
}

impl PersistMergedRecordsOptions {
    /// 构造默认入库选项。
    ///
    /// 调度器从数据库来源表触发时应传入准确的 `stat_date` 和 `feishu_source_id`；
    /// 手动调试入口没有来源 ID 时，数据库持久化步骤可以选择跳过。
    pub fn new(
        content_config_id: i64,
        content_type: impl Into<String>,
        feishu_source_id: Option<i64>,
        stat_date: NaiveDate,
    ) -> Self {
        Self {
            content_config_id,
            content_type: content_type.into(),
            feishu_source_id,
            stat_date,
            is_daily_final: false,
            trace_enabled: true,
            manual_source_value: DEFAULT_MANUAL_DATA_SOURCE.to_string(),
            manual_auto_approve: true,
            manual_auto_approve_result: DEFAULT_MANUAL_AUTO_APPROVE_RESULT.to_string(),
        }
    }
}

/// 待导入的星图飞书来源。
#[derive(Debug, Clone)]
pub struct PendingFeishuSource {
    pub feishu_source_id: i64,
    pub activity_period_id: i64,
    pub content_config_id: i64,
    pub content_type: String,
    pub feishu_sheet_url: String,
    pub stat_date: NaiveDate,
    pub is_daily_final: bool,
    pub trace_enabled: bool,
    pub attempt_count: i32,
}

#[derive(Debug, Clone, Default, Serialize, ToSchema)]
pub struct PersistMergedRecordsResult {
    pub persisted_rows: usize,
    pub quarantined_rows: usize,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct PendingImportFailure {
    pub feishu_source_id: i64,
    pub error: String,
    pub dead_lettered: bool,
}

#[derive(Debug, Clone, Default, Serialize, ToSchema)]
pub struct PendingImportResult {
    pub discovered_sources: usize,
    pub imported_sources: usize,
    pub partial_sources: usize,
    pub failed_sources: usize,
    pub dead_lettered_sources: usize,
    pub persisted_rows: usize,
    pub quarantined_rows: usize,
    pub failures: Vec<PendingImportFailure>,
}

/// 数据入库仓储。
#[derive(Debug, Clone)]
pub struct XingtuDataImportRepository {
    pool: PgPool,
}

impl XingtuDataImportRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// 查询待补偿导入的飞书来源。
    ///
    /// 这覆盖“已经获取了链接，但是还没有拉 Sheet 数据入库”的补偿场景。
    /// 已失败但未导入的来源也会被纳入，方便修复解析逻辑后自动重试。
    pub async fn list_pending_feishu_sources(
        &self,
        limit: i64,
        activity_period_id: Option<i64>,
    ) -> anyhow::Result<Vec<PendingFeishuSource>> {
        let rows = sqlx::query(
            r#"
            SELECT
                feishu_source_id,
                c.activity_period_id,
                s.content_config_id,
                s.content_type::text AS content_type,
                feishu_sheet_url,
                stat_date,
                is_daily_final,
                c.trace_enabled,
                s.attempt_count
            FROM xingtu_feishu_source s
            JOIN xingtu_activity_content_config c
                ON c.content_config_id = s.content_config_id
            WHERE
                s.is_imported = false
                AND s.import_status IN ('pending', 'failed')
                AND s.ignored_at IS NULL
                AND s.dead_letter_at IS NULL
                AND (s.next_retry_at IS NULL OR s.next_retry_at <= now())
                AND ($2::bigint IS NULL OR c.activity_period_id = $2)
            ORDER BY
                CASE s.import_status
                    WHEN 'pending' THEN 0
                    ELSE 1
                END,
                pulled_at,
                feishu_source_id
            LIMIT $1
            "#,
        )
        .bind(limit.max(1))
        .bind(activity_period_id)
        .fetch_all(&self.pool)
        .await
        .context("查询待导入飞书来源失败")?;

        rows.into_iter()
            .map(|row| {
                Ok(PendingFeishuSource {
                    feishu_source_id: row.try_get("feishu_source_id")?,
                    activity_period_id: row.try_get("activity_period_id")?,
                    content_config_id: row.try_get("content_config_id")?,
                    content_type: row.try_get("content_type")?,
                    feishu_sheet_url: row.try_get("feishu_sheet_url")?,
                    stat_date: row.try_get("stat_date")?,
                    is_daily_final: row.try_get("is_daily_final")?,
                    trace_enabled: row.try_get("trace_enabled")?,
                    attempt_count: row.try_get("attempt_count")?,
                })
            })
            .collect()
    }

    async fn get_pending_feishu_source(
        &self,
        feishu_source_id: i64,
    ) -> anyhow::Result<Option<PendingFeishuSource>> {
        let row = sqlx::query(
            r#"
            SELECT s.feishu_source_id, c.activity_period_id, s.content_config_id,
                s.content_type::text AS content_type, s.feishu_sheet_url, s.stat_date,
                s.is_daily_final, c.trace_enabled, s.attempt_count
            FROM xingtu_feishu_source s
            JOIN xingtu_activity_content_config c
                ON c.content_config_id = s.content_config_id
            WHERE s.feishu_source_id = $1 AND s.is_imported = false
            "#,
        )
        .bind(feishu_source_id)
        .fetch_optional(&self.pool)
        .await?;
        row.map(|row| {
            Ok(PendingFeishuSource {
                feishu_source_id: row.try_get("feishu_source_id")?,
                activity_period_id: row.try_get("activity_period_id")?,
                content_config_id: row.try_get("content_config_id")?,
                content_type: row.try_get("content_type")?,
                feishu_sheet_url: row.try_get("feishu_sheet_url")?,
                stat_date: row.try_get("stat_date")?,
                is_daily_final: row.try_get("is_daily_final")?,
                trace_enabled: row.try_get("trace_enabled")?,
                attempt_count: row.try_get("attempt_count")?,
            })
        })
        .transpose()
    }

    /// 持久化合并后的数据，并根据内容类型写入结构化业务表。
    pub async fn persist_merged_records(
        &self,
        options: PersistMergedRecordsOptions,
        rows: Vec<MergedRecordForDb>,
    ) -> anyhow::Result<PersistMergedRecordsResult> {
        if rows.is_empty() {
            return Ok(PersistMergedRecordsResult::default());
        }

        let mut tx = self.pool.begin().await.context("开启数据入库事务失败")?;
        let mut result = PersistMergedRecordsResult::default();

        for row in &rows {
            let required_time_field = match options.content_type.as_str() {
                "video" => "发布时间",
                "live" => "开播时间",
                other => return Err(anyhow!("未知 content_type：{other}")),
            };
            if field_timestamp(&row.fields, required_time_field).is_none() {
                sqlx::query(
                    r#"
                    INSERT INTO xingtu_data_quarantine (
                        feishu_source_id, content_config_id, content_type, unique_key,
                        reason_code, reason_message, raw_fields
                    )
                    VALUES ($1, $2, $3::xingtu_content_type, $4, 'missing_business_time', $5, $6)
                    ON CONFLICT DO NOTHING
                    "#,
                )
                .bind(options.feishu_source_id)
                .bind(options.content_config_id)
                .bind(&options.content_type)
                .bind(&row.unique_key)
                .bind(format!(
                    "缺少或无法解析必填业务时间字段 `{required_time_field}`"
                ))
                .bind(Value::Object(row.fields.clone()))
                .execute(&mut *tx)
                .await
                .with_context(|| format!("隔离异常数据失败：{}", row.unique_key))?;
                // 即使该异常行已在隔离区中存在，本轮仍然遇到了一条无效业务数据。
                result.quarantined_rows += 1;
                continue;
            }
            match options.content_type.as_str() {
                "video" => {
                    self.upsert_video_record(&mut tx, &options, row).await?;

                    // 手动独有视频也写播放量；同稿件冲突已在合并阶段优先保留星图行。
                    if options.trace_enabled && options.feishu_source_id.is_some() {
                        self.upsert_video_daily_metric(&mut tx, &options, row)
                            .await?;
                        self.insert_video_metric_import_history(&mut tx, &options, row)
                            .await?;
                    }
                }
                "live" => {
                    self.upsert_live_record(&mut tx, &options, row).await?;
                }
                other => return Err(anyhow!("未知 content_type：{other}")),
            }
            sqlx::query(
                r#"
                UPDATE xingtu_data_quarantine
                SET resolved_at = now()
                WHERE content_config_id = $1
                    AND content_type = $2::xingtu_content_type
                    AND unique_key = $3
                    AND resolved_at IS NULL
                "#,
            )
            .bind(options.content_config_id)
            .bind(&options.content_type)
            .bind(&row.unique_key)
            .execute(&mut *tx)
            .await
            .with_context(|| format!("标记已修复隔离数据失败：{}", row.unique_key))?;
            result.persisted_rows += 1;
        }

        tx.commit().await.context("提交数据入库事务失败")?;
        Ok(result)
    }

    /// 按内容配置和业务唯一键批量回写非空审核结果。
    pub async fn update_audit_results(
        &self,
        content_config_id: i64,
        content_type: &str,
        updates: &[AuditResultUpdate],
    ) -> anyhow::Result<usize> {
        if updates.is_empty() {
            return Ok(0);
        }

        let statement = match content_type {
            "video" => {
                r#"
                UPDATE video_content AS target
                SET
                    audit_result = source.audit_result,
                    label = source.label,
                    audit_extra = source.audit_extra,
                    updated_at = now()
                FROM unnest($2::text[], $3::text[], $4::text[], $5::jsonb[])
                    AS source(unique_key, audit_result, label, audit_extra)
                WHERE
                    target.content_config_id = $1
                    AND target.video_id = source.unique_key
                    AND (
                        target.audit_result IS DISTINCT FROM source.audit_result
                        OR target.label IS DISTINCT FROM source.label
                        OR target.audit_extra IS DISTINCT FROM source.audit_extra
                    )
                "#
            }
            "live" => {
                r#"
                UPDATE live_session AS target
                SET
                    audit_result = source.audit_result,
                    label = source.label,
                    audit_extra = source.audit_extra,
                    updated_at = now()
                FROM unnest($2::text[], $3::text[], $4::text[], $5::jsonb[])
                    AS source(unique_key, audit_result, label, audit_extra)
                WHERE
                    target.content_config_id = $1
                    AND target.live_room_id = source.unique_key
                    AND (
                        target.audit_result IS DISTINCT FROM source.audit_result
                        OR target.label IS DISTINCT FROM source.label
                        OR target.audit_extra IS DISTINCT FROM source.audit_extra
                    )
                "#
            }
            other => return Err(anyhow!("未知 content_type：{other}")),
        };
        let mut tx = self
            .pool
            .begin()
            .await
            .context("开启审核结果回写事务失败")?;
        let mut updated_rows = 0_usize;

        for chunk in updates.chunks(AUDIT_RESULT_UPDATE_BATCH_SIZE) {
            let unique_keys = chunk
                .iter()
                .map(|update| update.unique_key.clone())
                .collect::<Vec<_>>();
            let audit_results = chunk
                .iter()
                .map(|update| update.audit_result.clone())
                .collect::<Vec<_>>();
            let audit_extras = chunk
                .iter()
                .map(|update| Json(update.audit_extra.clone()))
                .collect::<Vec<_>>();
            let result = match content_type {
                "video" => {
                    let labels = chunk
                        .iter()
                        .map(|update| update.label.clone())
                        .collect::<Vec<_>>();
                    sqlx::query(statement)
                        .bind(content_config_id)
                        .bind(unique_keys)
                        .bind(audit_results)
                        .bind(labels)
                        .bind(audit_extras)
                        .execute(&mut *tx)
                        .await
                }
                "live" => {
                    let labels = chunk
                        .iter()
                        .map(|update| update.label.clone())
                        .collect::<Vec<_>>();
                    sqlx::query(statement)
                        .bind(content_config_id)
                        .bind(unique_keys)
                        .bind(audit_results)
                        .bind(labels)
                        .bind(audit_extras)
                        .execute(&mut *tx)
                        .await
                }
                _ => unreachable!("content_type 已在事务开始前校验"),
            }
            .with_context(|| {
                format!(
                    "批量回写审核结果失败：content_config_id={content_config_id} content_type={content_type}"
                )
            })?;
            updated_rows += usize::try_from(result.rows_affected())
                .context("审核结果更新行数超出 usize 范围")?;
        }

        tx.commit().await.context("提交审核结果回写事务失败")?;
        Ok(updated_rows)
    }

    /// 标记飞书来源导入成功。
    pub async fn mark_feishu_source_imported(
        &self,
        feishu_source_id: i64,
        imported_row_count: i32,
        partial: bool,
    ) -> anyhow::Result<()> {
        sqlx::query(
            r#"
            UPDATE xingtu_feishu_source
            SET
                is_imported = true,
                import_status = CASE WHEN $3 THEN 'partial'::xingtu_import_status ELSE 'imported'::xingtu_import_status END,
                imported_row_count = $2,
                imported_at = now(),
                error_message = NULL,
                next_retry_at = NULL,
                dead_letter_at = NULL
            WHERE feishu_source_id = $1
            "#,
        )
        .bind(feishu_source_id)
        .bind(imported_row_count)
        .bind(partial)
        .execute(&self.pool)
        .await
        .with_context(|| format!("标记飞书来源导入成功失败：{feishu_source_id}"))?;

        Ok(())
    }

    /// 标记飞书来源导入失败。
    pub async fn mark_feishu_source_failed(
        &self,
        feishu_source_id: i64,
        error_message: &str,
    ) -> anyhow::Result<bool> {
        let dead_lettered: bool = sqlx::query_scalar(
            r#"
            UPDATE xingtu_feishu_source
            SET
                is_imported = false,
                import_status = 'failed',
                error_message = $2,
                attempt_count = attempt_count + 1,
                last_attempt_at = now(),
                next_retry_at = CASE
                    WHEN attempt_count + 1 >= 8 THEN NULL
                    ELSE now() + make_interval(secs => LEAST(86400, 300 * power(2, LEAST(attempt_count, 8)))::int)
                END,
                dead_letter_at = CASE WHEN attempt_count + 1 >= 8 THEN now() ELSE NULL END
            WHERE feishu_source_id = $1
            RETURNING dead_letter_at IS NOT NULL
            "#,
        )
        .bind(feishu_source_id)
        .bind(error_message.chars().take(4_000).collect::<String>())
        .fetch_one(&self.pool)
        .await
        .with_context(|| format!("标记飞书来源导入失败失败：{feishu_source_id}"))?;

        Ok(dead_lettered)
    }

    pub async fn retry_failed_source(&self, feishu_source_id: i64) -> anyhow::Result<bool> {
        let result = sqlx::query(
            r#"
            UPDATE xingtu_feishu_source
            SET attempt_count = 0, next_retry_at = now(), dead_letter_at = NULL,
                ignored_at = NULL, import_status = 'pending', error_message = NULL
            WHERE feishu_source_id = $1 AND is_imported = false
            "#,
        )
        .bind(feishu_source_id)
        .execute(&self.pool)
        .await?;
        Ok(result.rows_affected() > 0)
    }

    pub async fn ignore_failed_source(&self, feishu_source_id: i64) -> anyhow::Result<bool> {
        let result = sqlx::query(
            "UPDATE xingtu_feishu_source SET ignored_at = now(), next_retry_at = NULL WHERE feishu_source_id = $1 AND is_imported = false",
        )
        .bind(feishu_source_id)
        .execute(&self.pool)
        .await?;
        Ok(result.rows_affected() > 0)
    }

    async fn upsert_video_record(
        &self,
        tx: &mut Transaction<'_, Postgres>,
        options: &PersistMergedRecordsOptions,
        row: &MergedRecordForDb,
    ) -> anyhow::Result<()> {
        let audit_result = audit_result_for_row(row, options);

        sqlx::query(
            r#"
            INSERT INTO video_content (
                content_config_id,
                video_id,
                first_feishu_source_id,
                publish_time,
                author_name,
                author_uid,
                title,
                relevance_review,
                award_level,
                award_amount,
                submit_org_id,
                submit_org_name,
                latest_video_url,
                latest_video_url_updated_at,
                audit_result,
                last_seen_at
            )
            VALUES (
                $1,
                $2,
                $3,
                $4,
                $5,
                $6,
                $7,
                $8,
                $9,
                $10,
                $11,
                $12,
                $13,
                CASE WHEN $13::text IS NULL THEN NULL ELSE now() END,
                $14,
                now()
            )
            ON CONFLICT (content_config_id, video_id)
            DO UPDATE SET
                first_feishu_source_id = COALESCE(video_content.first_feishu_source_id, EXCLUDED.first_feishu_source_id),
                author_name = EXCLUDED.author_name,
                author_uid = EXCLUDED.author_uid,
                title = EXCLUDED.title,
                relevance_review = EXCLUDED.relevance_review,
                award_level = EXCLUDED.award_level,
                award_amount = EXCLUDED.award_amount,
                submit_org_id = EXCLUDED.submit_org_id,
                submit_org_name = EXCLUDED.submit_org_name,
                latest_video_url = EXCLUDED.latest_video_url,
                latest_video_url_updated_at = CASE
                    WHEN EXCLUDED.latest_video_url IS DISTINCT FROM video_content.latest_video_url THEN now()
                    ELSE video_content.latest_video_url_updated_at
                END,
                audit_result = CASE
                    WHEN EXCLUDED.audit_result IS NOT NULL THEN EXCLUDED.audit_result
                    ELSE video_content.audit_result
                END,
                last_seen_at = now()
            "#,
        )
        .bind(options.content_config_id)
        .bind(&row.unique_key)
        .bind(options.feishu_source_id)
        .bind(
            field_timestamp(&row.fields, "发布时间")
                .expect("发布时间已在事务循环入口校验"),
        )
        .bind(field_string(&row.fields, "作者名称"))
        .bind(field_string(&row.fields, "作者uid"))
        .bind(field_string(&row.fields, "标题"))
        .bind(field_string(&row.fields, "相关性审核"))
        .bind(field_i32(&row.fields, "获奖等级"))
        .bind(field_non_negative_f64(&row.fields, "获奖金额"))
        .bind(field_string(&row.fields, "投稿机构ID"))
        .bind(field_string(&row.fields, "投稿机构名称"))
        .bind(field_string(&row.fields, "链接(视频链接有效期为1小时)"))
        .bind(audit_result)
        .execute(&mut **tx)
        .await
        .with_context(|| format!("写入视频结构化数据失败：{}", row.unique_key))?;

        Ok(())
    }

    async fn upsert_video_daily_metric(
        &self,
        tx: &mut Transaction<'_, Postgres>,
        options: &PersistMergedRecordsOptions,
        row: &MergedRecordForDb,
    ) -> anyhow::Result<()> {
        let feishu_source_id = options
            .feishu_source_id
            .ok_or_else(|| anyhow!("写入视频追踪表需要 feishu_source_id：{}", row.unique_key))?;
        let play_count = video_play_count_for_row(row, &options.manual_source_value);

        sqlx::query(
            r#"
            INSERT INTO video_daily_metric (
                content_config_id,
                feishu_source_id,
                stat_date,
                video_id,
                play_count,
                valid_play_count,
                like_count,
                valid_like_count,
                comment_count,
                share_count,
                component_click_count,
                android_activate_count,
                ios_activate_count,
                reservation_success_count,
                reservation_install_complete_count,
                follower_increase_count,
                like_rate,
                comment_rate,
                is_daily_final,
                pulled_at,
                imported_at
            )
            VALUES (
                $1,
                $2,
                $3,
                $4,
                $5,
                $6,
                $7,
                $8,
                $9,
                $10,
                $11,
                $12,
                $13,
                $14,
                $15,
                $16,
                $17,
                $18,
                $19,
                now(),
                now()
            )
            ON CONFLICT (content_config_id, stat_date, video_id)
            DO UPDATE SET
                feishu_source_id = EXCLUDED.feishu_source_id,
                play_count = EXCLUDED.play_count,
                valid_play_count = EXCLUDED.valid_play_count,
                like_count = EXCLUDED.like_count,
                valid_like_count = EXCLUDED.valid_like_count,
                comment_count = EXCLUDED.comment_count,
                share_count = EXCLUDED.share_count,
                component_click_count = EXCLUDED.component_click_count,
                android_activate_count = EXCLUDED.android_activate_count,
                ios_activate_count = EXCLUDED.ios_activate_count,
                reservation_success_count = EXCLUDED.reservation_success_count,
                reservation_install_complete_count = EXCLUDED.reservation_install_complete_count,
                follower_increase_count = EXCLUDED.follower_increase_count,
                like_rate = EXCLUDED.like_rate,
                comment_rate = EXCLUDED.comment_rate,
                is_daily_final = EXCLUDED.is_daily_final,
                pulled_at = now(),
                imported_at = now()
            "#,
        )
        .bind(options.content_config_id)
        .bind(feishu_source_id)
        .bind(options.stat_date)
        .bind(&row.unique_key)
        .bind(play_count)
        .bind(field_non_negative_i64(&row.fields, "有效播放量"))
        .bind(field_non_negative_i64(&row.fields, "点赞量"))
        .bind(field_non_negative_i64(&row.fields, "有效点赞量"))
        .bind(field_non_negative_i64(&row.fields, "评论量"))
        .bind(field_non_negative_i64(&row.fields, "分享量"))
        .bind(field_non_negative_i64(&row.fields, "组件点击数(延迟1天)"))
        .bind(field_non_negative_i64(&row.fields, "Android激活数"))
        .bind(field_non_negative_i64(&row.fields, "ios激活数"))
        .bind(field_non_negative_i64(&row.fields, "预约成功次数(仅安卓)"))
        .bind(field_non_negative_i64(&row.fields, "预约安装完成次数"))
        .bind(field_non_negative_i64(&row.fields, "涨粉数"))
        .bind(field_non_negative_f64(&row.fields, "点赞率"))
        .bind(field_non_negative_f64(&row.fields, "评论率"))
        .bind(options.is_daily_final)
        .execute(&mut **tx)
        .await
        .with_context(|| format!("写入视频每日追踪数据失败：{}", row.unique_key))?;

        Ok(())
    }

    async fn insert_video_metric_import_history(
        &self,
        tx: &mut Transaction<'_, Postgres>,
        options: &PersistMergedRecordsOptions,
        row: &MergedRecordForDb,
    ) -> anyhow::Result<()> {
        let feishu_source_id = options
            .feishu_source_id
            .ok_or_else(|| anyhow!("写入视频追踪历史需要 feishu_source_id：{}", row.unique_key))?;
        let play_count = video_play_count_for_row(row, &options.manual_source_value);

        sqlx::query(
            r#"
            INSERT INTO video_daily_metric_import_history (
                feishu_source_id,
                content_config_id,
                stat_date,
                video_id,
                play_count,
                valid_play_count,
                like_count,
                valid_like_count,
                comment_count,
                share_count,
                component_click_count,
                android_activate_count,
                ios_activate_count,
                reservation_success_count,
                reservation_install_complete_count,
                follower_increase_count,
                like_rate,
                comment_rate,
                imported_at
            )
            VALUES (
                $1,
                $2,
                $3,
                $4,
                $5,
                $6,
                $7,
                $8,
                $9,
                $10,
                $11,
                $12,
                $13,
                $14,
                $15,
                $16,
                $17,
                $18,
                now()
            )
            ON CONFLICT (feishu_source_id, video_id)
            DO UPDATE SET
                content_config_id = EXCLUDED.content_config_id,
                stat_date = EXCLUDED.stat_date,
                play_count = EXCLUDED.play_count,
                valid_play_count = EXCLUDED.valid_play_count,
                like_count = EXCLUDED.like_count,
                valid_like_count = EXCLUDED.valid_like_count,
                comment_count = EXCLUDED.comment_count,
                share_count = EXCLUDED.share_count,
                component_click_count = EXCLUDED.component_click_count,
                android_activate_count = EXCLUDED.android_activate_count,
                ios_activate_count = EXCLUDED.ios_activate_count,
                reservation_success_count = EXCLUDED.reservation_success_count,
                reservation_install_complete_count = EXCLUDED.reservation_install_complete_count,
                follower_increase_count = EXCLUDED.follower_increase_count,
                like_rate = EXCLUDED.like_rate,
                comment_rate = EXCLUDED.comment_rate,
                imported_at = now()
            "#,
        )
        .bind(feishu_source_id)
        .bind(options.content_config_id)
        .bind(options.stat_date)
        .bind(&row.unique_key)
        .bind(play_count)
        .bind(field_non_negative_i64(&row.fields, "有效播放量"))
        .bind(field_non_negative_i64(&row.fields, "点赞量"))
        .bind(field_non_negative_i64(&row.fields, "有效点赞量"))
        .bind(field_non_negative_i64(&row.fields, "评论量"))
        .bind(field_non_negative_i64(&row.fields, "分享量"))
        .bind(field_non_negative_i64(&row.fields, "组件点击数(延迟1天)"))
        .bind(field_non_negative_i64(&row.fields, "Android激活数"))
        .bind(field_non_negative_i64(&row.fields, "ios激活数"))
        .bind(field_non_negative_i64(&row.fields, "预约成功次数(仅安卓)"))
        .bind(field_non_negative_i64(&row.fields, "预约安装完成次数"))
        .bind(field_non_negative_i64(&row.fields, "涨粉数"))
        .bind(field_non_negative_f64(&row.fields, "点赞率"))
        .bind(field_non_negative_f64(&row.fields, "评论率"))
        .execute(&mut **tx)
        .await
        .with_context(|| format!("写入视频每日追踪历史失败：{}", row.unique_key))?;

        Ok(())
    }

    async fn upsert_live_record(
        &self,
        tx: &mut Transaction<'_, Postgres>,
        options: &PersistMergedRecordsOptions,
        row: &MergedRecordForDb,
    ) -> anyhow::Result<()> {
        let audit_result = audit_result_for_row(row, options);

        sqlx::query(
            r#"
            INSERT INTO live_session (
                content_config_id,
                feishu_source_id,
                live_room_id,
                start_time,
                anchor_name,
                anchor_uid,
                title,
                cumulative_viewer_count,
                comment_count,
                share_count,
                component_click_count,
                android_download_or_activate_count,
                ios_download_or_activate_count,
                live_url,
                live_url_updated_at,
                award_amount,
                live_exposure_pv,
                exposure_uv,
                acu,
                pcu,
                live_duration_seconds,
                avg_watch_duration_seconds,
                follower_increase_count,
                like_rate,
                comment_rate,
                live_game_name,
                audit_result,
                pulled_at,
                last_seen_at
            )
            VALUES (
                $1,
                $2,
                $3,
                $4,
                $5,
                $6,
                $7,
                $8,
                $9,
                $10,
                $11,
                $12,
                $13,
                $14,
                CASE WHEN $14::text IS NULL THEN NULL ELSE now() END,
                $15,
                $16,
                $17,
                $18,
                $19,
                $20,
                $21,
                $22,
                $23,
                $24,
                $25,
                $26,
                now(),
                now()
            )
            ON CONFLICT (content_config_id, live_room_id)
            DO UPDATE SET
                feishu_source_id = EXCLUDED.feishu_source_id,
                anchor_name = EXCLUDED.anchor_name,
                anchor_uid = EXCLUDED.anchor_uid,
                title = EXCLUDED.title,
                cumulative_viewer_count = EXCLUDED.cumulative_viewer_count,
                comment_count = EXCLUDED.comment_count,
                share_count = EXCLUDED.share_count,
                component_click_count = EXCLUDED.component_click_count,
                android_download_or_activate_count = EXCLUDED.android_download_or_activate_count,
                ios_download_or_activate_count = EXCLUDED.ios_download_or_activate_count,
                live_url = EXCLUDED.live_url,
                live_url_updated_at = CASE
                    WHEN EXCLUDED.live_url IS DISTINCT FROM live_session.live_url THEN now()
                    ELSE live_session.live_url_updated_at
                END,
                award_amount = EXCLUDED.award_amount,
                live_exposure_pv = EXCLUDED.live_exposure_pv,
                exposure_uv = EXCLUDED.exposure_uv,
                acu = COALESCE(EXCLUDED.acu, live_session.acu),
                pcu = EXCLUDED.pcu,
                live_duration_seconds = EXCLUDED.live_duration_seconds,
                avg_watch_duration_seconds = EXCLUDED.avg_watch_duration_seconds,
                follower_increase_count = EXCLUDED.follower_increase_count,
                like_rate = EXCLUDED.like_rate,
                comment_rate = EXCLUDED.comment_rate,
                live_game_name = EXCLUDED.live_game_name,
                audit_result = CASE
                    WHEN EXCLUDED.audit_result IS NOT NULL THEN EXCLUDED.audit_result
                    ELSE live_session.audit_result
                END,
                pulled_at = now(),
                last_seen_at = now()
            "#,
        )
        .bind(options.content_config_id)
        .bind(options.feishu_source_id)
        .bind(&row.unique_key)
        .bind(field_timestamp(&row.fields, "开播时间").expect("开播时间已在事务循环入口校验"))
        .bind(field_string(&row.fields, "主播名称"))
        .bind(field_string(&row.fields, "主播uid"))
        .bind(field_string(&row.fields, "标题"))
        .bind(field_non_negative_i64(&row.fields, "累计观看人数"))
        .bind(field_non_negative_i64(&row.fields, "评论量"))
        .bind(field_non_negative_i64(&row.fields, "分享量"))
        .bind(field_non_negative_i64(&row.fields, "组件点击数(延迟1天)"))
        .bind(field_non_negative_i64(
            &row.fields,
            "Android下载数/Android激活数",
        ))
        .bind(field_non_negative_i64(&row.fields, "ios下载数/ios激活数"))
        .bind(field_string(&row.fields, "链接(视频链接有效期为1小时)"))
        .bind(field_non_negative_f64(&row.fields, "获奖金额"))
        // 星图直播的业务“场观PV”直接使用 `直播曝光pv`，不要再猜 `观看人次PV`。
        .bind(field_non_negative_i64(&row.fields, "直播曝光pv"))
        .bind(field_non_negative_i64(&row.fields, "曝光uv"))
        .bind(field_non_negative_f64(&row.fields, "Acu"))
        .bind(field_non_negative_i64(&row.fields, "Pcu"))
        .bind(field_non_negative_i64(&row.fields, "直播时长"))
        .bind(field_non_negative_i64(&row.fields, "人均观看时长"))
        .bind(field_non_negative_i64(&row.fields, "涨粉数"))
        .bind(field_non_negative_f64(&row.fields, "点赞率"))
        .bind(field_non_negative_f64(&row.fields, "评论率"))
        .bind(field_string(&row.fields, "直播游戏名称"))
        .bind(audit_result)
        .execute(&mut **tx)
        .await
        .with_context(|| format!("写入直播结构化数据失败：{}", row.unique_key))?;

        Ok(())
    }
}

/// 扫描 pending 飞书来源，读取 Sheet 后写入数据库。
pub async fn import_pending_feishu_sources(
    repo: &XingtuDataImportRepository,
    lark: &LarkClient,
    limit: i64,
    activity_period_id: Option<i64>,
) -> anyhow::Result<PendingImportResult> {
    let pending_sources = repo
        .list_pending_feishu_sources(limit, activity_period_id)
        .await?;
    let mut summary = PendingImportResult {
        discovered_sources: pending_sources.len(),
        ..PendingImportResult::default()
    };

    for source in pending_sources {
        process_pending_source(repo, lark, &source, &mut summary).await?;
    }

    Ok(summary)
}

/// 管理端按来源 ID 精确重试，不受自动队列退避时间和 dead-letter 状态限制。
pub async fn import_feishu_source_by_id(
    repo: &XingtuDataImportRepository,
    lark: &LarkClient,
    feishu_source_id: i64,
) -> anyhow::Result<PendingImportResult> {
    let source = repo
        .get_pending_feishu_source(feishu_source_id)
        .await?
        .ok_or_else(|| anyhow!("未找到可重试来源：{feishu_source_id}"))?;
    let mut summary = PendingImportResult {
        discovered_sources: 1,
        ..PendingImportResult::default()
    };
    process_pending_source(repo, lark, &source, &mut summary).await?;
    if let Some(failure) = summary.failures.first() {
        return Err(anyhow!(
            "来源 {} 重试失败：{}",
            failure.feishu_source_id,
            failure.error
        ));
    }
    Ok(summary)
}

async fn process_pending_source(
    repo: &XingtuDataImportRepository,
    lark: &LarkClient,
    source: &PendingFeishuSource,
    summary: &mut PendingImportResult,
) -> anyhow::Result<()> {
    match import_single_pending_source(repo, lark, source).await {
        Ok(result) => {
            let partial = result.quarantined_rows > 0;
            repo.mark_feishu_source_imported(
                source.feishu_source_id,
                i32::try_from(result.persisted_rows).unwrap_or(i32::MAX),
                partial,
            )
            .await?;
            summary.imported_sources += 1;
            summary.partial_sources += usize::from(partial);
            summary.persisted_rows += result.persisted_rows;
            summary.quarantined_rows += result.quarantined_rows;
        }
        Err(error) => {
            let error_text = format!("{error:#}");
            let dead_lettered = repo
                .mark_feishu_source_failed(source.feishu_source_id, &error_text)
                .await?;
            summary.failed_sources += 1;
            summary.dead_lettered_sources += usize::from(dead_lettered);
            summary.failures.push(PendingImportFailure {
                feishu_source_id: source.feishu_source_id,
                error: error_text.chars().take(1_000).collect(),
                dead_lettered,
            });
            tracing::error!(
                feishu_source_id = source.feishu_source_id,
                activity_period_id = source.activity_period_id,
                attempt_count = source.attempt_count + 1,
                dead_lettered,
                error = ?error,
                "单个飞书来源导入失败，继续处理队列后续来源"
            );
        }
    }
    Ok(())
}

async fn import_single_pending_source(
    repo: &XingtuDataImportRepository,
    lark: &LarkClient,
    source: &PendingFeishuSource,
) -> anyhow::Result<PersistMergedRecordsResult> {
    let spreadsheet_token = get_spreadsheet_token(&source.feishu_sheet_url)
        .context("解析 pending source spreadsheet_token 失败")?;
    let rows = read_first_sheet_rows(lark, &spreadsheet_token)
        .await
        .context("读取 pending source Sheet 失败")?;
    let rules = field_rules_for_content_type(&source.content_type)?;
    let unique_key_field = unique_key_for_content_type(&source.content_type)?;
    let mut source_rows = parse_sheet_rows(rows, unique_key_field, &rules)
        .context("解析 pending source Sheet 行失败")?;

    for row in &mut source_rows {
        row.fields.insert(
            "数据来源".to_string(),
            Value::String(XINGTU_DATA_SOURCE.to_string()),
        );
    }

    let records = source_rows
        .into_iter()
        .map(MergedRecordForDb::from)
        .collect::<Vec<_>>();
    let mut options = PersistMergedRecordsOptions::new(
        source.content_config_id,
        source.content_type.clone(),
        Some(source.feishu_source_id),
        source.stat_date,
    );
    options.is_daily_final = source.is_daily_final;
    options.trace_enabled = source.trace_enabled;

    repo.persist_merged_records(options, records).await
}

/// 判断是否来自手动登记。
///
/// 数据库不保存“数据来源”字段本身，但入库时需要用它区分：
/// - 星图数据：按原始字段写入全部视频指标，审核结果为空
/// - 手动登记：播放量缺失时写 0，审核结果可自动通过
fn is_manual_data_source(row: &MergedRecordForDb, manual_source_value: &str) -> bool {
    normalize_name(&row.data_source) == normalize_name(manual_source_value)
}

/// 手动登记的视频必须产生明确的播放量快照，空值按 0 处理。
fn video_play_count_for_row(row: &MergedRecordForDb, manual_source_value: &str) -> Option<i64> {
    let play_count = field_non_negative_i64(&row.fields, "播放量");
    if is_manual_data_source(row, manual_source_value) {
        Some(play_count.unwrap_or(0))
    } else {
        play_count
    }
}

/// 根据来源计算入库审核结果。
fn audit_result_for_row(
    row: &MergedRecordForDb,
    options: &PersistMergedRecordsOptions,
) -> Option<String> {
    if !options.manual_auto_approve {
        return None;
    }

    if is_manual_data_source(row, &options.manual_source_value) {
        Some(options.manual_auto_approve_result.clone())
    } else {
        None
    }
}

fn unique_key_for_content_type(content_type: &str) -> anyhow::Result<&'static str> {
    match content_type {
        "video" => Ok(VIDEO_UNIQUE_KEY_FIELD),
        "live" => Ok(LIVE_UNIQUE_KEY_FIELD),
        other => Err(anyhow!("未知 content_type：{other}")),
    }
}

fn field_rules_for_content_type(content_type: &str) -> anyhow::Result<FieldValueRules> {
    match content_type {
        "video" => Ok(video_field_rules_for_db()),
        "live" => Ok(live_field_rules_for_db()),
        other => Err(anyhow!("未知 content_type：{other}")),
    }
}

fn video_field_rules_for_db() -> FieldValueRules {
    FieldValueRules::new()
        .with_skip_fields(&[
            "修改人",
            "创建人",
            "创建时间",
            "最后更新时间",
            "更新时间",
            "未添加稿件",
            "record_id",
            "recordurl",
            "sharedurl",
        ])
        .with_url_fields(&["链接(视频链接有效期为1小时)"])
        .with_text_fields(&[
            VIDEO_UNIQUE_KEY_FIELD,
            "作者uid",
            "作者名称",
            "标题",
            "相关性审核",
            "投稿机构ID",
            "投稿机构名称",
            "数据来源",
        ])
        .with_number_fields(&[
            "播放量",
            "点赞量",
            "评论量",
            "分享量",
            "有效播放量",
            "有效点赞量",
            "组件点击数(延迟1天)",
            "获奖等级",
            "获奖金额",
            "Android激活数",
            "ios激活数",
            "预约成功次数(仅安卓)",
            "预约安装完成次数",
            "涨粉数",
            "点赞率",
            "评论率",
        ])
        .with_timestamp_millis_fields(&["发布时间"])
}

fn live_field_rules_for_db() -> FieldValueRules {
    FieldValueRules::new()
        .with_skip_fields(&[
            "修改人",
            "创建人",
            "创建时间",
            "最后更新时间",
            "更新时间",
            "未添加稿件",
            "record_id",
            "recordurl",
            "sharedurl",
        ])
        .with_url_fields(&["链接(视频链接有效期为1小时)"])
        .with_text_fields(&[
            LIVE_UNIQUE_KEY_FIELD,
            "主播uid",
            "主播名称",
            "标题",
            "直播游戏名称",
            "数据来源",
        ])
        .with_number_fields(&[
            "累计观看人数",
            "评论量",
            "分享量",
            "组件点击数(延迟1天)",
            "获奖金额",
            "直播曝光pv",
            "曝光uv",
            "Acu",
            "Pcu",
            "直播时长",
            "人均观看时长",
            "涨粉数",
            "Android下载数/Android激活数",
            "ios下载数/ios激活数",
            "点赞率",
            "评论率",
        ])
        .with_timestamp_millis_fields(&["开播时间"])
}

fn field_value<'a>(fields: &'a Map<String, Value>, field_name: &str) -> Option<&'a Value> {
    fields.iter().find_map(|(name, value)| {
        if normalize_name(name) == normalize_name(field_name) {
            Some(value)
        } else {
            None
        }
    })
}

fn field_string(fields: &Map<String, Value>, field_name: &str) -> Option<String> {
    field_value(fields, field_name).and_then(value_to_plain_string)
}

fn field_i32(fields: &Map<String, Value>, field_name: &str) -> Option<i32> {
    field_i64(fields, field_name).and_then(|value| i32::try_from(value).ok())
}

fn field_i64(fields: &Map<String, Value>, field_name: &str) -> Option<i64> {
    let value = field_value(fields, field_name)?;

    if let Some(value) = value.as_i64() {
        return Some(value);
    }

    if let Some(value) = value.as_f64() {
        return Some(value.round() as i64);
    }

    value_to_plain_string(value)?
        .replace(',', "")
        .trim_end_matches(".0")
        .parse::<i64>()
        .ok()
}

fn field_non_negative_i64(fields: &Map<String, Value>, field_name: &str) -> Option<i64> {
    field_i64(fields, field_name).filter(|value| *value >= 0)
}

fn field_f64(fields: &Map<String, Value>, field_name: &str) -> Option<f64> {
    let value = field_value(fields, field_name)?;

    if let Some(value) = value.as_f64() {
        return Some(value);
    }

    value_to_plain_string(value)?
        .replace(',', "")
        .trim_end_matches('%')
        .parse::<f64>()
        .ok()
}

fn field_non_negative_f64(fields: &Map<String, Value>, field_name: &str) -> Option<f64> {
    field_f64(fields, field_name).filter(|value| *value >= 0.0)
}

fn field_timestamp(fields: &Map<String, Value>, field_name: &str) -> Option<NaiveDateTime> {
    let value = field_value(fields, field_name)?;

    if let Some(millis) = value.as_i64() {
        return Shanghai
            .timestamp_millis_opt(millis)
            .single()
            .map(|dt| dt.naive_local());
    }

    let text = value_to_plain_string(value)?;
    parse_naive_datetime(&text)
}

fn parse_naive_datetime(value: &str) -> Option<NaiveDateTime> {
    let value = value.trim();

    NaiveDateTime::parse_from_str(value, "%Y-%m-%d %H:%M:%S")
        .ok()
        .or_else(|| NaiveDateTime::parse_from_str(value, "%Y/%m/%d %H:%M:%S").ok())
        .or_else(|| {
            NaiveDateTime::parse_from_str(&format!("{value} 00:00:00"), "%Y-%m-%d %H:%M:%S").ok()
        })
}

fn normalize_name(value: &str) -> String {
    value
        .trim()
        .replace(['\r', '\n', ' ', '\u{3000}'], "")
        .to_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn fields(values: &[(&str, Value)]) -> Map<String, Value> {
        values
            .iter()
            .map(|(name, value)| ((*name).to_string(), value.clone()))
            .collect()
    }

    #[test]
    fn field_non_negative_i64_keeps_valid_numbers() {
        let fields = fields(&[
            ("播放量", json!(1234)),
            ("评论量", json!("1,235")),
            ("分享量", json!(12.4)),
        ]);

        assert_eq!(field_non_negative_i64(&fields, "播放量"), Some(1234));
        assert_eq!(field_non_negative_i64(&fields, "评论量"), Some(1235));
        assert_eq!(field_non_negative_i64(&fields, "分享量"), Some(12));
    }

    #[test]
    fn field_non_negative_i64_treats_negative_values_as_empty() {
        let fields = fields(&[
            ("组件点击数(延迟1天)", json!(-1)),
            ("Android下载数/Android激活数", json!("-2")),
        ]);

        assert_eq!(field_non_negative_i64(&fields, "组件点击数(延迟1天)"), None);
        assert_eq!(
            field_non_negative_i64(&fields, "Android下载数/Android激活数"),
            None
        );
    }

    #[test]
    fn field_non_negative_f64_keeps_percent_text_and_drops_negative() {
        let fields = fields(&[("点赞率", json!("12.5%")), ("评论率", json!(-0.1))]);

        assert_eq!(field_non_negative_f64(&fields, "点赞率"), Some(12.5));
        assert_eq!(field_non_negative_f64(&fields, "评论率"), None);
    }

    #[test]
    fn manual_video_play_count_defaults_to_zero_only_when_missing() {
        let missing = MergedRecordForDb {
            unique_key: "manual-missing".to_string(),
            data_source: "手动登记".to_string(),
            fields: Map::new(),
        };
        let concrete = MergedRecordForDb {
            unique_key: "manual-concrete".to_string(),
            data_source: "手动登记".to_string(),
            fields: fields(&[("播放量", json!(1234))]),
        };

        assert_eq!(video_play_count_for_row(&missing, "手动登记"), Some(0));
        assert_eq!(video_play_count_for_row(&concrete, "手动登记"), Some(1234));
    }

    #[test]
    fn xingtu_video_play_count_keeps_missing_value_as_null() {
        let row = MergedRecordForDb {
            unique_key: "xingtu-missing".to_string(),
            data_source: "星图数据".to_string(),
            fields: Map::new(),
        };

        assert_eq!(video_play_count_for_row(&row, "手动登记"), None);
    }

    #[test]
    fn missing_or_invalid_business_time_is_not_silently_replaced() {
        let fields = Map::new();
        assert!(field_timestamp(&fields, "发布时间").is_none());
        assert!(field_timestamp(&fields, "开播时间").is_none());

        let fields = Map::from_iter([(
            "发布时间".to_owned(),
            Value::String("invalid timestamp".to_owned()),
        )]);
        assert!(field_timestamp(&fields, "发布时间").is_none());
    }
}
