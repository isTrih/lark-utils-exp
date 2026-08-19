use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use chrono::{DateTime, NaiveDate, NaiveDateTime, Utc};
use salvo::oapi::{ToParameters, ToSchema};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sqlx::{PgPool, Row};
use std::collections::BTreeMap;

/// 分页参数。
#[derive(Debug, Clone, Default, Deserialize, Serialize, ToParameters, ToSchema)]
#[salvo(parameters(default_parameter_in = Query))]
pub struct PageQuery {
    /// 返回条数，服务端限制为 1..500。
    pub limit: Option<i64>,
    /// 偏移量，小于 0 时按 0 处理。
    pub offset: Option<i64>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize, ToParameters, ToSchema)]
#[salvo(parameters(default_parameter_in = Query))]
pub struct OperationalContentQuery {
    pub activity_period_id: Option<i64>,
    pub content_config_id: Option<i64>,
    pub status: Option<String>,
    /// 审核标签，忽略大小写精确匹配；`search` 仍支持对标签做模糊搜索。
    pub label: Option<String>,
    /// 北京时间业务日期开始；视频/直播按当天 00:00:00 起算。
    pub date_from: Option<NaiveDate>,
    /// 北京时间业务日期结束；包含当天 23:59:59 及其小数秒。
    pub date_to: Option<NaiveDate>,
    pub search: Option<String>,
    pub limit: Option<i64>,
    pub offset: Option<i64>,
    /// 新接口游标；存在时优先于 offset。
    pub cursor: Option<String>,
}

impl OperationalContentQuery {
    pub fn limit(&self) -> i64 {
        self.limit.unwrap_or(50).clamp(1, 500)
    }

    pub fn resolved_offset(&self) -> anyhow::Result<i64> {
        let Some(cursor) = self
            .cursor
            .as_deref()
            .filter(|value| !value.trim().is_empty())
        else {
            return Ok(self.offset.unwrap_or(0).max(0));
        };
        let decoded = URL_SAFE_NO_PAD
            .decode(cursor.trim())
            .map_err(|_| anyhow::anyhow!("cursor 不是有效的分页游标"))?;
        let text = std::str::from_utf8(&decoded)
            .map_err(|_| anyhow::anyhow!("cursor 不是有效的分页游标"))?;
        text.parse::<i64>()
            .ok()
            .filter(|offset| *offset >= 0)
            .ok_or_else(|| anyhow::anyhow!("cursor 不是有效的分页游标"))
    }

    pub fn validate(&self) -> anyhow::Result<()> {
        validate_optional_date_range(self.date_from, self.date_to)
    }
}

#[derive(Debug, Clone, Deserialize, Serialize, ToSchema)]
pub struct PageMeta {
    pub total: i64,
    pub has_more: bool,
    pub next_cursor: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize, ToSchema)]
pub struct PagedResponse<T> {
    pub ok: bool,
    pub data: Vec<T>,
    pub meta: PageMeta,
}

impl PageQuery {
    pub fn limit(&self) -> i64 {
        self.limit.unwrap_or(50).clamp(1, 500)
    }

    pub fn offset(&self) -> i64 {
        self.offset.unwrap_or(0).max(0)
    }
}

#[derive(Debug, Clone, Deserialize, Serialize, ToSchema)]
pub struct ActivityPeriodDto {
    pub activity_period_id: i64,
    pub project_id: i64,
    pub project_key: String,
    pub project_display_name: String,
    pub period: String,
    pub period_code: Option<String>,
    pub xingtu_account_id: Option<String>,
    pub task_month: NaiveDate,
    pub bitable_url: String,
    pub need_trace: bool,
    pub morning_review_enabled: bool,
    pub periodic_sync_enabled: bool,
    pub periodic_sync_interval_hours: i32,
    pub tracking_start_date: Option<NaiveDate>,
    pub tracking_end_date: Option<NaiveDate>,
}

/// 当前启用项目的精简信息。
#[derive(Debug, Clone, Deserialize, Serialize, ToSchema)]
pub struct CurrentProjectDto {
    pub activity_period_id: i64,
    pub project_id: i64,
    pub project_key: String,
    pub project_display_name: String,
    pub period: String,
}

#[derive(Debug, Clone, Deserialize, Serialize, ToSchema)]
pub struct ContentConfigDto {
    pub content_config_id: i64,
    pub activity_period_id: i64,
    pub period: String,
    pub content_type: String,
    pub xingtu_task_id: String,
    pub xingtu_task_name: Option<String>,
    pub source_spreadsheet_url: Option<String>,
    pub manual_table_id: Option<String>,
    pub main_table_id: String,
    pub audit_table_id: String,
    pub sync_enabled: bool,
    pub trace_enabled: bool,
}

#[derive(Debug, Clone, Deserialize, Serialize, ToSchema)]
pub struct FeishuSourceDto {
    pub feishu_source_id: i64,
    pub content_config_id: i64,
    pub content_type: String,
    pub feishu_sheet_url: String,
    pub trigger_type: String,
    pub stat_date: NaiveDate,
    pub pulled_at: DateTime<Utc>,
    pub is_daily_final: bool,
    pub import_status: String,
    pub imported_row_count: i32,
}

#[derive(Debug, Clone, Deserialize, Serialize, ToSchema)]
pub struct PendingSummaryDto {
    pub pending_sources: i64,
    pub failed_sources: i64,
}

#[derive(Debug, Clone, Deserialize, Serialize, ToSchema)]
pub struct VideoContentDto {
    pub content_config_id: i64,
    pub video_id: String,
    pub publish_time: NaiveDateTime,
    pub author_name: Option<String>,
    pub author_uid: Option<String>,
    pub title: Option<String>,
    pub audit_result: Option<String>,
    pub label: Option<String>,
    /// 审核扩展字段；顶层机密字段 `key` 永不返回。
    pub audit_extra: Value,
    pub last_seen_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Deserialize, Serialize, ToSchema)]
pub struct VideoMetricDto {
    pub content_config_id: i64,
    pub stat_date: NaiveDate,
    pub video_id: String,
    pub play_count: Option<i64>,
    pub valid_play_count: Option<i64>,
    pub like_count: Option<i64>,
    pub comment_count: Option<i64>,
    pub share_count: Option<i64>,
    pub is_daily_final: bool,
    pub imported_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Deserialize, Serialize, ToSchema)]
pub struct VideoTraceMetricDto {
    pub feishu_source_id: i64,
    pub content_config_id: i64,
    pub stat_date: NaiveDate,
    pub video_id: String,
    pub trigger_type: String,
    pub pulled_at: DateTime<Utc>,
    pub is_daily_final: bool,
    pub play_count: Option<i64>,
    pub valid_play_count: Option<i64>,
    pub like_count: Option<i64>,
    pub valid_like_count: Option<i64>,
    pub comment_count: Option<i64>,
    pub share_count: Option<i64>,
    pub component_click_count: Option<i64>,
    pub android_activate_count: Option<i64>,
    pub ios_activate_count: Option<i64>,
    pub reservation_success_count: Option<i64>,
    pub reservation_install_complete_count: Option<i64>,
    pub follower_increase_count: Option<i64>,
    pub like_rate: Option<f64>,
    pub comment_rate: Option<f64>,
    pub imported_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Deserialize, Serialize, ToSchema)]
pub struct LiveSessionDto {
    pub content_config_id: i64,
    pub live_room_id: String,
    pub start_time: NaiveDateTime,
    pub anchor_name: Option<String>,
    pub anchor_uid: Option<String>,
    pub title: Option<String>,
    pub cumulative_viewer_count: Option<i64>,
    pub live_exposure_pv: Option<i64>,
    pub acu: Option<f64>,
    pub audit_result: Option<String>,
    /// 审核扩展字段；顶层机密字段 `key` 永不返回。
    pub audit_extra: Value,
    pub last_seen_at: DateTime<Utc>,
}

/// 视频分析查询参数。
#[derive(Debug, Clone, Deserialize, Serialize, ToParameters, ToSchema)]
#[salvo(parameters(default_parameter_in = Query))]
pub struct VideoAnalyticsQuery {
    /// 内容配置 ID；用于限定某一期活动的直播/视频配置。
    pub content_config_id: Option<i64>,
    /// 视频 ID。
    pub video_id: Option<String>,
    /// 作者 uid，精确匹配。
    pub author_uid: Option<String>,
    /// 作者名称，模糊匹配。
    pub author_name: Option<String>,
    /// 审核标签，忽略大小写精确匹配。
    pub label: Option<String>,
    /// 指定单日；传入后优先于 date_from/date_to。
    pub date: Option<NaiveDate>,
    /// 日期区间开始，闭区间。
    pub date_from: Option<NaiveDate>,
    /// 日期区间结束，闭区间。
    pub date_to: Option<NaiveDate>,
    /// 返回视频条数，服务端限制为 1..500。
    pub limit: Option<i64>,
    /// 视频分页偏移量。
    pub offset: Option<i64>,
}

impl VideoAnalyticsQuery {
    pub fn limit(&self) -> i64 {
        self.limit.unwrap_or(50).clamp(1, 500)
    }

    pub fn offset(&self) -> i64 {
        self.offset.unwrap_or(0).max(0)
    }

    pub fn validate(&self) -> anyhow::Result<()> {
        if self.date.is_none() {
            validate_optional_date_range(self.date_from, self.date_to)?;
        }
        Ok(())
    }
}

/// 直播分析查询参数。
#[derive(Debug, Clone, Deserialize, Serialize, ToParameters, ToSchema)]
#[salvo(parameters(default_parameter_in = Query))]
pub struct LiveAnalyticsQuery {
    /// 内容配置 ID；用于限定某一期活动的直播配置。
    pub content_config_id: Option<i64>,
    /// 主播 uid，精确匹配。
    pub anchor_uid: Option<String>,
    /// 主播名称，模糊匹配。
    pub anchor_name: Option<String>,
    /// 指定单日；传入后优先于 date_from/date_to。
    pub date: Option<NaiveDate>,
    /// 日期区间开始，闭区间。
    pub date_from: Option<NaiveDate>,
    /// 日期区间结束，闭区间。
    pub date_to: Option<NaiveDate>,
}

impl LiveAnalyticsQuery {
    pub fn validate(&self) -> anyhow::Result<()> {
        if self.date.is_none() {
            validate_optional_date_range(self.date_from, self.date_to)?;
        }
        Ok(())
    }
}

/// 视频增长榜查询参数。
#[derive(Debug, Clone, Deserialize, Serialize, ToParameters, ToSchema)]
#[salvo(parameters(default_parameter_in = Query))]
pub struct VideoGrowthQuery {
    /// 内容配置 ID；用于限定某一期活动的视频配置。
    pub content_config_id: Option<i64>,
    /// 作者 uid，精确匹配。
    pub author_uid: Option<String>,
    /// 作者名称，模糊匹配。
    pub author_name: Option<String>,
    /// 审核标签，忽略大小写精确匹配。
    pub label: Option<String>,
    /// 指定单日；传入后优先于 date_from/date_to。
    pub date: Option<NaiveDate>,
    /// 日期区间开始，闭区间。
    pub date_from: Option<NaiveDate>,
    /// 日期区间结束，闭区间。
    pub date_to: Option<NaiveDate>,
    /// 返回榜单条数，服务端限制为 1..100。
    pub limit: Option<i64>,
}

impl VideoGrowthQuery {
    pub fn limit(&self) -> i64 {
        self.limit.unwrap_or(20).clamp(1, 100)
    }

    pub fn validate(&self) -> anyhow::Result<()> {
        if self.date.is_none() {
            validate_optional_date_range(self.date_from, self.date_to)?;
        }
        Ok(())
    }
}

/// 按审核标签生成周报汇总的查询参数。
#[derive(Debug, Clone, Deserialize, Serialize, ToParameters, ToSchema)]
#[salvo(parameters(default_parameter_in = Query))]
pub struct VideoLabelSummaryQuery {
    /// 活动期次 ID。
    pub activity_period_id: Option<i64>,
    /// 视频内容配置 ID。
    pub content_config_id: Option<i64>,
    /// 审核结果，精确匹配。
    pub status: Option<String>,
    /// 只返回指定审核标签，忽略大小写精确匹配。
    pub label: Option<String>,
    /// 北京时间开始日期，必填，从当天 00:00:00 起算。
    pub date_from: NaiveDate,
    /// 北京时间结束日期，必填，包含当天 23:59:59 及其小数秒。
    pub date_to: NaiveDate,
}

impl VideoLabelSummaryQuery {
    pub fn validate(&self) -> anyhow::Result<()> {
        validate_required_date_range(self.date_from, self.date_to)?;
        let days = self
            .date_to
            .signed_duration_since(self.date_from)
            .num_days()
            + 1;
        anyhow::ensure!(days <= 366, "日期范围不能超过 366 天");
        Ok(())
    }
}

/// 单个视频的一天指标点。
#[derive(Debug, Clone, Deserialize, Serialize, ToSchema)]
pub struct VideoMetricPointDto {
    pub stat_date: NaiveDate,
    pub play_count: Option<i64>,
    pub valid_play_count: Option<i64>,
    pub like_count: Option<i64>,
    pub comment_count: Option<i64>,
    pub share_count: Option<i64>,
    pub play_increment: Option<i64>,
    pub like_increment: Option<i64>,
    pub comment_increment: Option<i64>,
    pub is_daily_final: bool,
    pub imported_at: DateTime<Utc>,
}

/// 视频基础信息和线性每日指标。
#[derive(Debug, Clone, Deserialize, Serialize, ToSchema)]
pub struct VideoWithMetricsDto {
    pub content_config_id: i64,
    pub video_id: String,
    pub publish_time: NaiveDateTime,
    pub author_name: Option<String>,
    pub author_uid: Option<String>,
    pub title: Option<String>,
    pub audit_result: Option<String>,
    pub label: Option<String>,
    /// 审核扩展字段；顶层机密字段 `key` 永不返回。
    pub audit_extra: Value,
    pub latest_play_count: Option<i64>,
    pub latest_like_count: Option<i64>,
    pub latest_comment_count: Option<i64>,
    pub metrics: Vec<VideoMetricPointDto>,
}

/// 视频聚合数据。
#[derive(Debug, Clone, Deserialize, Serialize, ToSchema)]
pub struct VideoSummaryDto {
    /// 查询范围内按每日增量口径汇总的播放量。
    pub play_count: i64,
    /// 查询范围内按每日增量口径汇总的点赞量。
    pub like_count: i64,
    /// 查询范围内按每日增量口径汇总的评论量。
    pub comment_count: i64,
    /// 查询范围内出现过指标数据的稿件数量。
    pub content_count: i64,
    /// 查询范围内出现过指标数据的作者数量。
    pub author_count: i64,
}

/// 周报查询实际使用的北京时间范围。
#[derive(Debug, Clone, Deserialize, Serialize, ToSchema)]
pub struct BeijingReportRangeDto {
    pub timezone: String,
    pub date_from: NaiveDate,
    pub date_to: NaiveDate,
    pub start_at: NaiveDateTime,
    pub end_at: NaiveDateTime,
}

/// 单个审核标签在日期范围内的视频周报数据。
#[derive(Debug, Clone, Deserialize, Serialize, ToSchema)]
pub struct VideoLabelAggregateDto {
    /// 原始审核标签；`null` 表示未标注。
    pub label: Option<String>,
    /// 直接用于报表展示的标签名，空标签统一为“未标注”。
    pub label_name: String,
    /// 发布时间位于查询北京时间范围内的稿件数。
    pub published_video_count: i64,
    /// 查询日期范围内出现过指标变化的稿件数。
    pub active_video_count: i64,
    /// 上述发布或活跃稿件涉及的去重作者数。
    pub author_count: i64,
    /// 查询日期范围内按每日快照差值累计的播放增量。
    pub play_growth: i64,
    pub valid_play_growth: i64,
    pub like_growth: i64,
    pub comment_growth: i64,
    pub share_growth: i64,
}

/// 所有标签之和；稿件属于唯一标签，因此这些字段可以安全求和。
#[derive(Debug, Clone, Deserialize, Serialize, ToSchema)]
pub struct VideoLabelTotalsDto {
    pub published_video_count: i64,
    pub active_video_count: i64,
    pub play_growth: i64,
    pub valid_play_growth: i64,
    pub like_growth: i64,
    pub comment_growth: i64,
    pub share_growth: i64,
}

#[derive(Debug, Clone, Deserialize, Serialize, ToSchema)]
pub struct VideoLabelSummaryResponse {
    pub ok: bool,
    pub range: BeijingReportRangeDto,
    pub data: Vec<VideoLabelAggregateDto>,
    pub total: VideoLabelTotalsDto,
}

/// 直播聚合数据。
#[derive(Debug, Clone, Deserialize, Serialize, ToSchema)]
pub struct LiveSummaryDto {
    /// 场观 PV；星图来源字段为 `直播曝光pv`。
    pub live_exposure_pv: i64,
    /// ACU 平均值。
    pub avg_acu: Option<f64>,
    /// 直播场次数量。
    pub live_session_count: i64,
    /// 主播数量。
    pub anchor_count: i64,
}

/// 播放量增长较快的视频。
#[derive(Debug, Clone, Deserialize, Serialize, ToSchema)]
pub struct VideoGrowthDto {
    pub content_config_id: i64,
    pub video_id: String,
    pub publish_time: NaiveDateTime,
    pub author_name: Option<String>,
    pub author_uid: Option<String>,
    pub title: Option<String>,
    pub audit_result: Option<String>,
    pub label: Option<String>,
    /// 审核扩展字段；顶层机密字段 `key` 永不返回。
    pub audit_extra: Value,
    pub play_growth: i64,
    pub avg_daily_play_growth: Option<f64>,
    pub metric_days: i64,
    pub latest_play_count: Option<i64>,
    pub latest_stat_date: Option<NaiveDate>,
}

pub async fn list_activity_periods(pool: &PgPool) -> anyhow::Result<Vec<ActivityPeriodDto>> {
    let rows = sqlx::query(
        r#"
        SELECT
            period.activity_period_id,
            period.project_id,
            project.project_key,
            project.display_name AS project_display_name,
            period.period,
            period.period_code,
            period.xingtu_account_id,
            period.task_month,
            period.bitable_url,
            period.need_trace,
            period.morning_review_enabled,
            period.periodic_sync_enabled,
            period.periodic_sync_interval_hours,
            period.tracking_start_date,
            period.tracking_end_date
        FROM xingtu_activity_period period
        JOIN xingtu_project project ON project.project_id = period.project_id
        WHERE period.is_active = true AND project.is_active = true
        ORDER BY period.task_month DESC, period.period
        "#,
    )
    .fetch_all(pool)
    .await?;

    rows.into_iter()
        .map(|row| {
            Ok(ActivityPeriodDto {
                activity_period_id: row.try_get("activity_period_id")?,
                project_id: row.try_get("project_id")?,
                project_key: row.try_get("project_key")?,
                project_display_name: row.try_get("project_display_name")?,
                period: row.try_get("period")?,
                period_code: row.try_get("period_code")?,
                xingtu_account_id: row.try_get("xingtu_account_id")?,
                task_month: row.try_get("task_month")?,
                bitable_url: row.try_get("bitable_url")?,
                need_trace: row.try_get("need_trace")?,
                morning_review_enabled: row.try_get("morning_review_enabled")?,
                periodic_sync_enabled: row.try_get("periodic_sync_enabled")?,
                periodic_sync_interval_hours: row.try_get("periodic_sync_interval_hours")?,
                tracking_start_date: row.try_get("tracking_start_date")?,
                tracking_end_date: row.try_get("tracking_end_date")?,
            })
        })
        .collect()
}

pub async fn list_current_projects(pool: &PgPool) -> anyhow::Result<Vec<CurrentProjectDto>> {
    let rows = sqlx::query(
        r#"
        SELECT
            period.activity_period_id,
            period.project_id,
            project.project_key,
            project.display_name AS project_display_name,
            period.period
        FROM xingtu_activity_period period
        JOIN xingtu_project project ON project.project_id = period.project_id
        WHERE period.is_active = true AND project.is_active = true
        ORDER BY period.task_month DESC, period.activity_period_id DESC
        "#,
    )
    .fetch_all(pool)
    .await?;

    rows.into_iter()
        .map(|row| {
            Ok(CurrentProjectDto {
                activity_period_id: row.try_get("activity_period_id")?,
                project_id: row.try_get("project_id")?,
                project_key: row.try_get("project_key")?,
                project_display_name: row.try_get("project_display_name")?,
                period: row.try_get("period")?,
            })
        })
        .collect()
}

pub async fn list_content_configs(pool: &PgPool) -> anyhow::Result<Vec<ContentConfigDto>> {
    let rows = sqlx::query(
        r#"
        SELECT
            c.content_config_id,
            c.activity_period_id,
            p.period,
            c.content_type::text AS content_type,
            c.xingtu_task_id,
            c.xingtu_task_name,
            c.source_spreadsheet_url,
            c.manual_table_id,
            c.main_table_id,
            c.audit_table_id,
            c.sync_enabled,
            c.trace_enabled
        FROM xingtu_activity_content_config c
        JOIN xingtu_activity_period p
            ON p.activity_period_id = c.activity_period_id
        WHERE p.is_active = true
        ORDER BY p.task_month DESC, p.period, c.content_type
        "#,
    )
    .fetch_all(pool)
    .await?;

    rows.into_iter()
        .map(|row| {
            Ok(ContentConfigDto {
                content_config_id: row.try_get("content_config_id")?,
                activity_period_id: row.try_get("activity_period_id")?,
                period: row.try_get("period")?,
                content_type: row.try_get("content_type")?,
                xingtu_task_id: row.try_get("xingtu_task_id")?,
                xingtu_task_name: row.try_get("xingtu_task_name")?,
                source_spreadsheet_url: row.try_get("source_spreadsheet_url")?,
                manual_table_id: row.try_get("manual_table_id")?,
                main_table_id: row.try_get("main_table_id")?,
                audit_table_id: row.try_get("audit_table_id")?,
                sync_enabled: row.try_get("sync_enabled")?,
                trace_enabled: row.try_get("trace_enabled")?,
            })
        })
        .collect()
}

pub async fn list_feishu_sources(
    pool: &PgPool,
    page: PageQuery,
) -> anyhow::Result<Vec<FeishuSourceDto>> {
    let rows = sqlx::query(
        r#"
        SELECT
            feishu_source_id,
            content_config_id,
            content_type::text AS content_type,
            feishu_sheet_url,
            trigger_type::text AS trigger_type,
            stat_date,
            pulled_at,
            is_daily_final,
            import_status::text AS import_status,
            imported_row_count
        FROM xingtu_feishu_source
        ORDER BY pulled_at DESC, feishu_source_id DESC
        LIMIT $1 OFFSET $2
        "#,
    )
    .bind(page.limit())
    .bind(page.offset())
    .fetch_all(pool)
    .await?;

    rows.into_iter()
        .map(|row| {
            Ok(FeishuSourceDto {
                feishu_source_id: row.try_get("feishu_source_id")?,
                content_config_id: row.try_get("content_config_id")?,
                content_type: row.try_get("content_type")?,
                feishu_sheet_url: row.try_get("feishu_sheet_url")?,
                trigger_type: row.try_get("trigger_type")?,
                stat_date: row.try_get("stat_date")?,
                pulled_at: row.try_get("pulled_at")?,
                is_daily_final: row.try_get("is_daily_final")?,
                import_status: row.try_get("import_status")?,
                imported_row_count: row.try_get("imported_row_count")?,
            })
        })
        .collect()
}

pub async fn pending_summary(pool: &PgPool) -> anyhow::Result<PendingSummaryDto> {
    let row = sqlx::query(
        r#"
        SELECT
            COUNT(*) FILTER (
                WHERE is_imported = false AND import_status = 'pending'
                    AND ignored_at IS NULL AND dead_letter_at IS NULL
            ) AS pending_sources,
            COUNT(*) FILTER (
                WHERE is_imported = false AND import_status = 'failed'
                    AND ignored_at IS NULL AND dead_letter_at IS NULL
            ) AS failed_sources
        FROM xingtu_feishu_source
        "#,
    )
    .fetch_one(pool)
    .await?;

    Ok(PendingSummaryDto {
        pending_sources: row.try_get("pending_sources")?,
        failed_sources: row.try_get("failed_sources")?,
    })
}

pub async fn list_video_contents(
    pool: &PgPool,
    page: PageQuery,
) -> anyhow::Result<Vec<VideoContentDto>> {
    let rows = sqlx::query(
        r#"
        SELECT
            content_config_id,
            video_id,
            publish_time,
            author_name,
            author_uid,
            title,
            audit_result,
            label,
            audit_extra,
            last_seen_at
        FROM video_content
        ORDER BY last_seen_at DESC
        LIMIT $1 OFFSET $2
        "#,
    )
    .bind(page.limit())
    .bind(page.offset())
    .fetch_all(pool)
    .await?;

    rows.into_iter()
        .map(|row| {
            Ok(VideoContentDto {
                content_config_id: row.try_get("content_config_id")?,
                video_id: row.try_get("video_id")?,
                publish_time: row.try_get("publish_time")?,
                author_name: row.try_get("author_name")?,
                author_uid: row.try_get("author_uid")?,
                title: row.try_get("title")?,
                audit_result: row.try_get("audit_result")?,
                label: row.try_get("label")?,
                audit_extra: crate::server::audit_extra_query::redact_audit_extra(
                    row.try_get("audit_extra")?,
                ),
                last_seen_at: row.try_get("last_seen_at")?,
            })
        })
        .collect()
}

pub async fn list_video_metrics(
    pool: &PgPool,
    page: PageQuery,
) -> anyhow::Result<Vec<VideoMetricDto>> {
    let rows = sqlx::query(
        r#"
        SELECT
            content_config_id,
            stat_date,
            video_id,
            play_count,
            valid_play_count,
            like_count,
            comment_count,
            share_count,
            is_daily_final,
            imported_at
        FROM video_daily_metric
        ORDER BY stat_date DESC, imported_at DESC
        LIMIT $1 OFFSET $2
        "#,
    )
    .bind(page.limit())
    .bind(page.offset())
    .fetch_all(pool)
    .await?;

    rows.into_iter()
        .map(|row| {
            Ok(VideoMetricDto {
                content_config_id: row.try_get("content_config_id")?,
                stat_date: row.try_get("stat_date")?,
                video_id: row.try_get("video_id")?,
                play_count: row.try_get("play_count")?,
                valid_play_count: row.try_get("valid_play_count")?,
                like_count: row.try_get("like_count")?,
                comment_count: row.try_get("comment_count")?,
                share_count: row.try_get("share_count")?,
                is_daily_final: row.try_get("is_daily_final")?,
                imported_at: row.try_get("imported_at")?,
            })
        })
        .collect()
}

pub async fn list_video_trace_metrics(
    pool: &PgPool,
    query: VideoAnalyticsQuery,
) -> anyhow::Result<Vec<VideoTraceMetricDto>> {
    let rows = sqlx::query(
        r#"
        SELECT
            h.feishu_source_id,
            h.content_config_id,
            h.stat_date,
            h.video_id,
            s.trigger_type::text AS trigger_type,
            s.pulled_at,
            s.is_daily_final,
            h.play_count,
            h.valid_play_count,
            h.like_count,
            h.valid_like_count,
            h.comment_count,
            h.share_count,
            h.component_click_count,
            h.android_activate_count,
            h.ios_activate_count,
            h.reservation_success_count,
            h.reservation_install_complete_count,
            h.follower_increase_count,
            h.like_rate::float8 AS like_rate,
            h.comment_rate::float8 AS comment_rate,
            h.imported_at
        FROM video_daily_metric_import_history h
        JOIN xingtu_feishu_source s
            ON s.feishu_source_id = h.feishu_source_id
        JOIN video_content vc
            ON vc.content_config_id = h.content_config_id
            AND vc.video_id = h.video_id
        WHERE
            ($1::bigint IS NULL OR h.content_config_id = $1)
            AND ($2::text IS NULL OR h.video_id = $2)
            AND ($3::text IS NULL OR vc.author_uid = $3)
            AND ($4::text IS NULL OR vc.author_name ILIKE '%' || $4 || '%')
            AND ($5::text IS NULL OR lower(NULLIF(btrim(vc.label), '')) = lower($5))
            AND (
                ($6::date IS NOT NULL AND h.stat_date = $6)
                OR (
                    $6::date IS NULL
                    AND ($7::date IS NULL OR h.stat_date >= $7)
                    AND ($8::date IS NULL OR h.stat_date <= $8)
                )
            )
        ORDER BY s.pulled_at DESC, h.imported_at DESC, h.content_config_id, h.video_id
        LIMIT $9 OFFSET $10
        "#,
    )
    .bind(query.content_config_id)
    .bind(trim_optional_string(query.video_id.as_deref()))
    .bind(trim_optional_string(query.author_uid.as_deref()))
    .bind(trim_optional_string(query.author_name.as_deref()))
    .bind(trim_optional_string(query.label.as_deref()))
    .bind(query.date)
    .bind(query.date_from)
    .bind(query.date_to)
    .bind(query.limit())
    .bind(query.offset())
    .fetch_all(pool)
    .await?;

    rows.into_iter()
        .map(|row| {
            Ok(VideoTraceMetricDto {
                feishu_source_id: row.try_get("feishu_source_id")?,
                content_config_id: row.try_get("content_config_id")?,
                stat_date: row.try_get("stat_date")?,
                video_id: row.try_get("video_id")?,
                trigger_type: row.try_get("trigger_type")?,
                pulled_at: row.try_get("pulled_at")?,
                is_daily_final: row.try_get("is_daily_final")?,
                play_count: row.try_get("play_count")?,
                valid_play_count: row.try_get("valid_play_count")?,
                like_count: row.try_get("like_count")?,
                valid_like_count: row.try_get("valid_like_count")?,
                comment_count: row.try_get("comment_count")?,
                share_count: row.try_get("share_count")?,
                component_click_count: row.try_get("component_click_count")?,
                android_activate_count: row.try_get("android_activate_count")?,
                ios_activate_count: row.try_get("ios_activate_count")?,
                reservation_success_count: row.try_get("reservation_success_count")?,
                reservation_install_complete_count: row
                    .try_get("reservation_install_complete_count")?,
                follower_increase_count: row.try_get("follower_increase_count")?,
                like_rate: row.try_get("like_rate")?,
                comment_rate: row.try_get("comment_rate")?,
                imported_at: row.try_get("imported_at")?,
            })
        })
        .collect()
}

pub async fn list_live_sessions(
    pool: &PgPool,
    page: PageQuery,
) -> anyhow::Result<Vec<LiveSessionDto>> {
    let rows = sqlx::query(
        r#"
        SELECT
            content_config_id,
            live_room_id,
            start_time,
            anchor_name,
            anchor_uid,
            title,
            cumulative_viewer_count,
            live_exposure_pv,
            acu::float8 AS acu,
            audit_result,
            audit_extra,
            last_seen_at
        FROM live_session
        ORDER BY last_seen_at DESC
        LIMIT $1 OFFSET $2
        "#,
    )
    .bind(page.limit())
    .bind(page.offset())
    .fetch_all(pool)
    .await?;

    rows.into_iter()
        .map(|row| {
            Ok(LiveSessionDto {
                content_config_id: row.try_get("content_config_id")?,
                live_room_id: row.try_get("live_room_id")?,
                start_time: row.try_get("start_time")?,
                anchor_name: row.try_get("anchor_name")?,
                anchor_uid: row.try_get("anchor_uid")?,
                title: row.try_get("title")?,
                cumulative_viewer_count: row.try_get("cumulative_viewer_count")?,
                live_exposure_pv: row.try_get("live_exposure_pv")?,
                acu: row.try_get("acu")?,
                audit_result: row.try_get("audit_result")?,
                audit_extra: crate::server::audit_extra_query::redact_audit_extra(
                    row.try_get("audit_extra")?,
                ),
                last_seen_at: row.try_get("last_seen_at")?,
            })
        })
        .collect()
}

pub async fn list_videos_with_metrics(
    pool: &PgPool,
    query: VideoAnalyticsQuery,
) -> anyhow::Result<Vec<VideoWithMetricsDto>> {
    let rows = sqlx::query(
        r#"
        WITH series AS (
            SELECT
                vc.content_config_id,
                vc.video_id,
                vc.publish_time,
                vc.author_name,
                vc.author_uid,
                vc.title,
                vc.audit_result,
                vc.label,
                vc.audit_extra,
                m.stat_date,
                m.play_count,
                m.valid_play_count,
                m.like_count,
                m.comment_count,
                m.share_count,
                m.is_daily_final,
                m.imported_at,
                LAG(m.play_count) OVER (
                    PARTITION BY m.content_config_id, m.video_id
                    ORDER BY m.stat_date
                ) AS prev_play_count,
                LAG(m.like_count) OVER (
                    PARTITION BY m.content_config_id, m.video_id
                    ORDER BY m.stat_date
                ) AS prev_like_count,
                LAG(m.comment_count) OVER (
                    PARTITION BY m.content_config_id, m.video_id
                    ORDER BY m.stat_date
                ) AS prev_comment_count
            FROM video_content vc
            LEFT JOIN video_daily_metric m
                ON m.content_config_id = vc.content_config_id
                AND m.video_id = vc.video_id
            WHERE
                ($1::bigint IS NULL OR vc.content_config_id = $1)
                AND ($2::text IS NULL OR vc.video_id = $2)
                AND ($3::text IS NULL OR vc.author_uid = $3)
                AND ($4::text IS NULL OR vc.author_name ILIKE '%' || $4 || '%')
                AND ($5::text IS NULL OR lower(NULLIF(btrim(vc.label), '')) = lower($5))
        ),
        filtered AS (
            SELECT
                *,
                CASE
                    WHEN play_count IS NULL THEN NULL
                    ELSE GREATEST(play_count - COALESCE(prev_play_count, 0), 0)
                END AS play_increment,
                CASE
                    WHEN like_count IS NULL THEN NULL
                    ELSE GREATEST(like_count - COALESCE(prev_like_count, 0), 0)
                END AS like_increment,
                CASE
                    WHEN comment_count IS NULL THEN NULL
                    ELSE GREATEST(comment_count - COALESCE(prev_comment_count, 0), 0)
                END AS comment_increment
            FROM series
            WHERE
                stat_date IS NULL
                OR (
                    ($6::date IS NOT NULL AND stat_date = $6)
                    OR (
                        $6::date IS NULL
                        AND ($7::date IS NULL OR stat_date >= $7)
                        AND ($8::date IS NULL OR stat_date <= $8)
                    )
                )
        ),
        paged_videos AS (
            SELECT
                content_config_id,
                video_id,
                MAX(stat_date) AS latest_stat_date,
                MAX(imported_at) AS latest_imported_at
            FROM filtered
            GROUP BY content_config_id, video_id
            ORDER BY
                latest_stat_date DESC NULLS LAST,
                latest_imported_at DESC NULLS LAST,
                content_config_id,
                video_id
            LIMIT $9 OFFSET $10
        )
        SELECT f.*
        FROM filtered f
        JOIN paged_videos p
            ON p.content_config_id = f.content_config_id
            AND p.video_id = f.video_id
        ORDER BY
            p.latest_stat_date DESC NULLS LAST,
            p.latest_imported_at DESC NULLS LAST,
            f.content_config_id,
            f.video_id,
            f.stat_date NULLS LAST
        "#,
    )
    .bind(query.content_config_id)
    .bind(trim_optional_string(query.video_id.as_deref()))
    .bind(trim_optional_string(query.author_uid.as_deref()))
    .bind(trim_optional_string(query.author_name.as_deref()))
    .bind(trim_optional_string(query.label.as_deref()))
    .bind(query.date)
    .bind(query.date_from)
    .bind(query.date_to)
    .bind(query.limit())
    .bind(query.offset())
    .fetch_all(pool)
    .await?;

    let mut videos = Vec::<VideoWithMetricsDto>::new();
    let mut positions = BTreeMap::<(i64, String), usize>::new();

    for row in rows {
        let content_config_id: i64 = row.try_get("content_config_id")?;
        let video_id: String = row.try_get("video_id")?;
        let key = (content_config_id, video_id.clone());
        let index = if let Some(index) = positions.get(&key) {
            *index
        } else {
            let index = videos.len();
            positions.insert(key, index);
            videos.push(VideoWithMetricsDto {
                content_config_id,
                video_id,
                publish_time: row.try_get("publish_time")?,
                author_name: row.try_get("author_name")?,
                author_uid: row.try_get("author_uid")?,
                title: row.try_get("title")?,
                audit_result: row.try_get("audit_result")?,
                label: row.try_get("label")?,
                audit_extra: crate::server::audit_extra_query::redact_audit_extra(
                    row.try_get("audit_extra")?,
                ),
                latest_play_count: None,
                latest_like_count: None,
                latest_comment_count: None,
                metrics: Vec::new(),
            });
            index
        };

        let stat_date: Option<NaiveDate> = row.try_get("stat_date")?;
        if let Some(stat_date) = stat_date {
            let point = VideoMetricPointDto {
                stat_date,
                play_count: row.try_get("play_count")?,
                valid_play_count: row.try_get("valid_play_count")?,
                like_count: row.try_get("like_count")?,
                comment_count: row.try_get("comment_count")?,
                share_count: row.try_get("share_count")?,
                play_increment: row.try_get("play_increment")?,
                like_increment: row.try_get("like_increment")?,
                comment_increment: row.try_get("comment_increment")?,
                is_daily_final: row.try_get("is_daily_final")?,
                imported_at: row.try_get("imported_at")?,
            };

            // SQL 已按 stat_date 升序输出同一视频的指标点，循环中的最后一个点就是最新快照。
            videos[index].latest_play_count = point.play_count;
            videos[index].latest_like_count = point.like_count;
            videos[index].latest_comment_count = point.comment_count;
            videos[index].metrics.push(point);
        }
    }

    Ok(videos)
}

pub async fn video_summary(
    pool: &PgPool,
    query: VideoAnalyticsQuery,
) -> anyhow::Result<VideoSummaryDto> {
    let row = sqlx::query(
        r#"
        WITH series AS (
            SELECT
                vc.content_config_id,
                vc.video_id,
                vc.author_name,
                vc.author_uid,
                m.stat_date,
                m.play_count,
                m.like_count,
                m.comment_count,
                LAG(m.play_count) OVER (
                    PARTITION BY m.content_config_id, m.video_id
                    ORDER BY m.stat_date
                ) AS prev_play_count,
                LAG(m.like_count) OVER (
                    PARTITION BY m.content_config_id, m.video_id
                    ORDER BY m.stat_date
                ) AS prev_like_count,
                LAG(m.comment_count) OVER (
                    PARTITION BY m.content_config_id, m.video_id
                    ORDER BY m.stat_date
                ) AS prev_comment_count
            FROM video_content vc
            JOIN video_daily_metric m
                ON m.content_config_id = vc.content_config_id
                AND m.video_id = vc.video_id
            WHERE
                ($1::bigint IS NULL OR vc.content_config_id = $1)
                AND ($2::text IS NULL OR vc.video_id = $2)
                AND ($3::text IS NULL OR vc.author_uid = $3)
                AND ($4::text IS NULL OR vc.author_name ILIKE '%' || $4 || '%')
                AND ($5::text IS NULL OR lower(NULLIF(btrim(vc.label), '')) = lower($5))
        ),
        filtered AS (
            SELECT
                *,
                CASE
                    WHEN play_count IS NULL THEN NULL
                    ELSE GREATEST(play_count - COALESCE(prev_play_count, 0), 0)
                END AS play_increment,
                CASE
                    WHEN like_count IS NULL THEN NULL
                    ELSE GREATEST(like_count - COALESCE(prev_like_count, 0), 0)
                END AS like_increment,
                CASE
                    WHEN comment_count IS NULL THEN NULL
                    ELSE GREATEST(comment_count - COALESCE(prev_comment_count, 0), 0)
                END AS comment_increment
            FROM series
            WHERE
                (
                    ($6::date IS NOT NULL AND stat_date = $6)
                    OR (
                        $6::date IS NULL
                        AND ($7::date IS NULL OR stat_date >= $7)
                        AND ($8::date IS NULL OR stat_date <= $8)
                    )
                )
        )
        SELECT
            COALESCE(SUM(COALESCE(play_increment, 0)), 0)::bigint AS play_count,
            COALESCE(SUM(COALESCE(like_increment, 0)), 0)::bigint AS like_count,
            COALESCE(SUM(COALESCE(comment_increment, 0)), 0)::bigint AS comment_count,
            COUNT(DISTINCT (content_config_id, video_id))::bigint AS content_count,
            COUNT(DISTINCT COALESCE(NULLIF(author_uid, ''), NULLIF(author_name, '')))::bigint AS author_count
        FROM filtered
        "#,
    )
    .bind(query.content_config_id)
    .bind(trim_optional_string(query.video_id.as_deref()))
    .bind(trim_optional_string(query.author_uid.as_deref()))
    .bind(trim_optional_string(query.author_name.as_deref()))
    .bind(trim_optional_string(query.label.as_deref()))
    .bind(query.date)
    .bind(query.date_from)
    .bind(query.date_to)
    .fetch_one(pool)
    .await?;

    Ok(VideoSummaryDto {
        play_count: row.try_get("play_count")?,
        like_count: row.try_get("like_count")?,
        comment_count: row.try_get("comment_count")?,
        content_count: row.try_get("content_count")?,
        author_count: row.try_get("author_count")?,
    })
}

pub async fn video_label_summary(
    pool: &PgPool,
    query: VideoLabelSummaryQuery,
) -> anyhow::Result<VideoLabelSummaryResponse> {
    query.validate()?;
    let rows = sqlx::query(
        r#"
        WITH eligible_videos AS (
            SELECT
                vc.content_config_id,
                vc.video_id,
                vc.publish_time,
                NULLIF(btrim(vc.label), '') AS normalized_label,
                COALESCE(NULLIF(btrim(vc.author_uid), ''), NULLIF(btrim(vc.author_name), ''))
                    AS author_key
            FROM video_content vc
            JOIN xingtu_activity_content_config c
                ON c.content_config_id = vc.content_config_id
            WHERE
                ($1::bigint IS NULL OR c.activity_period_id = $1)
                AND ($2::bigint IS NULL OR vc.content_config_id = $2)
                AND ($3::text IS NULL OR vc.audit_result = $3)
                AND ($4::text IS NULL
                    OR lower(NULLIF(btrim(vc.label), '')) = lower($4))
        ),
        metric_series AS (
            SELECT
                ev.normalized_label,
                ev.content_config_id,
                ev.video_id,
                ev.author_key,
                m.stat_date,
                m.play_count,
                m.valid_play_count,
                m.like_count,
                m.comment_count,
                m.share_count,
                LAG(m.play_count) OVER (
                    PARTITION BY m.content_config_id, m.video_id ORDER BY m.stat_date
                ) AS prev_play_count,
                LAG(m.valid_play_count) OVER (
                    PARTITION BY m.content_config_id, m.video_id ORDER BY m.stat_date
                ) AS prev_valid_play_count,
                LAG(m.like_count) OVER (
                    PARTITION BY m.content_config_id, m.video_id ORDER BY m.stat_date
                ) AS prev_like_count,
                LAG(m.comment_count) OVER (
                    PARTITION BY m.content_config_id, m.video_id ORDER BY m.stat_date
                ) AS prev_comment_count,
                LAG(m.share_count) OVER (
                    PARTITION BY m.content_config_id, m.video_id ORDER BY m.stat_date
                ) AS prev_share_count
            FROM eligible_videos ev
            JOIN video_daily_metric m
                ON m.content_config_id = ev.content_config_id
                AND m.video_id = ev.video_id
            WHERE m.stat_date <= $6
        ),
        metric_range AS (
            SELECT
                normalized_label,
                content_config_id,
                video_id,
                author_key,
                GREATEST(COALESCE(play_count, 0) - COALESCE(prev_play_count, 0), 0)
                    AS play_growth,
                GREATEST(
                    COALESCE(valid_play_count, 0) - COALESCE(prev_valid_play_count, 0), 0
                ) AS valid_play_growth,
                GREATEST(COALESCE(like_count, 0) - COALESCE(prev_like_count, 0), 0)
                    AS like_growth,
                GREATEST(COALESCE(comment_count, 0) - COALESCE(prev_comment_count, 0), 0)
                    AS comment_growth,
                GREATEST(COALESCE(share_count, 0) - COALESCE(prev_share_count, 0), 0)
                    AS share_growth
            FROM metric_series
            WHERE stat_date >= $5 AND stat_date <= $6
        ),
        metric_aggregate AS (
            SELECT
                normalized_label,
                COUNT(DISTINCT (content_config_id, video_id))::bigint AS active_video_count,
                COALESCE(SUM(play_growth), 0)::bigint AS play_growth,
                COALESCE(SUM(valid_play_growth), 0)::bigint AS valid_play_growth,
                COALESCE(SUM(like_growth), 0)::bigint AS like_growth,
                COALESCE(SUM(comment_growth), 0)::bigint AS comment_growth,
                COALESCE(SUM(share_growth), 0)::bigint AS share_growth
            FROM metric_range
            GROUP BY normalized_label
        ),
        published_range AS (
            SELECT normalized_label, content_config_id, video_id, author_key
            FROM eligible_videos
            WHERE publish_time >= $5::date::timestamp
                AND publish_time < ($6::date + 1)::timestamp
        ),
        published_aggregate AS (
            SELECT normalized_label, COUNT(*)::bigint AS published_video_count
            FROM published_range
            GROUP BY normalized_label
        ),
        involved_videos AS (
            SELECT normalized_label, content_config_id, video_id, author_key FROM metric_range
            UNION
            SELECT normalized_label, content_config_id, video_id, author_key FROM published_range
        ),
        author_aggregate AS (
            SELECT normalized_label, COUNT(DISTINCT author_key)::bigint AS author_count
            FROM involved_videos
            GROUP BY normalized_label
        ),
        label_keys AS (
            SELECT normalized_label FROM metric_aggregate
            UNION
            SELECT normalized_label FROM published_aggregate
        )
        SELECT
            lk.normalized_label AS label,
            COALESCE(pa.published_video_count, 0)::bigint AS published_video_count,
            COALESCE(ma.active_video_count, 0)::bigint AS active_video_count,
            COALESCE(aa.author_count, 0)::bigint AS author_count,
            COALESCE(ma.play_growth, 0)::bigint AS play_growth,
            COALESCE(ma.valid_play_growth, 0)::bigint AS valid_play_growth,
            COALESCE(ma.like_growth, 0)::bigint AS like_growth,
            COALESCE(ma.comment_growth, 0)::bigint AS comment_growth,
            COALESCE(ma.share_growth, 0)::bigint AS share_growth
        FROM label_keys lk
        LEFT JOIN metric_aggregate ma
            ON ma.normalized_label IS NOT DISTINCT FROM lk.normalized_label
        LEFT JOIN published_aggregate pa
            ON pa.normalized_label IS NOT DISTINCT FROM lk.normalized_label
        LEFT JOIN author_aggregate aa
            ON aa.normalized_label IS NOT DISTINCT FROM lk.normalized_label
        ORDER BY
            COALESCE(ma.play_growth, 0) DESC,
            COALESCE(pa.published_video_count, 0) DESC,
            lk.normalized_label NULLS LAST
        "#,
    )
    .bind(query.activity_period_id)
    .bind(query.content_config_id)
    .bind(trim_optional_string(query.status.as_deref()))
    .bind(trim_optional_string(query.label.as_deref()))
    .bind(query.date_from)
    .bind(query.date_to)
    .fetch_all(pool)
    .await?;

    let data = rows
        .into_iter()
        .map(|row| {
            let label: Option<String> = row.try_get("label")?;
            Ok(VideoLabelAggregateDto {
                label_name: label.clone().unwrap_or_else(|| "未标注".to_owned()),
                label,
                published_video_count: row.try_get("published_video_count")?,
                active_video_count: row.try_get("active_video_count")?,
                author_count: row.try_get("author_count")?,
                play_growth: row.try_get("play_growth")?,
                valid_play_growth: row.try_get("valid_play_growth")?,
                like_growth: row.try_get("like_growth")?,
                comment_growth: row.try_get("comment_growth")?,
                share_growth: row.try_get("share_growth")?,
            })
        })
        .collect::<Result<Vec<_>, sqlx::Error>>()?;
    let total = data.iter().fold(
        VideoLabelTotalsDto {
            published_video_count: 0,
            active_video_count: 0,
            play_growth: 0,
            valid_play_growth: 0,
            like_growth: 0,
            comment_growth: 0,
            share_growth: 0,
        },
        |mut total, item| {
            total.published_video_count = total
                .published_video_count
                .saturating_add(item.published_video_count);
            total.active_video_count = total
                .active_video_count
                .saturating_add(item.active_video_count);
            total.play_growth = total.play_growth.saturating_add(item.play_growth);
            total.valid_play_growth = total
                .valid_play_growth
                .saturating_add(item.valid_play_growth);
            total.like_growth = total.like_growth.saturating_add(item.like_growth);
            total.comment_growth = total.comment_growth.saturating_add(item.comment_growth);
            total.share_growth = total.share_growth.saturating_add(item.share_growth);
            total
        },
    );

    Ok(VideoLabelSummaryResponse {
        ok: true,
        range: beijing_report_range(query.date_from, query.date_to),
        data,
        total,
    })
}

pub async fn live_summary(
    pool: &PgPool,
    query: LiveAnalyticsQuery,
) -> anyhow::Result<LiveSummaryDto> {
    let row = sqlx::query(
        r#"
        SELECT
            COALESCE(SUM(COALESCE(live_exposure_pv, 0)), 0)::bigint AS live_exposure_pv,
            AVG(acu)::float8 AS avg_acu,
            COUNT(*)::bigint AS live_session_count,
            COUNT(DISTINCT COALESCE(NULLIF(anchor_uid, ''), NULLIF(anchor_name, '')))::bigint AS anchor_count
        FROM live_session
        WHERE
            ($1::bigint IS NULL OR content_config_id = $1)
            AND ($2::text IS NULL OR anchor_uid = $2)
            AND ($3::text IS NULL OR anchor_name ILIKE '%' || $3 || '%')
            AND (
                ($4::date IS NOT NULL
                    AND start_time >= $4::date::timestamp
                    AND start_time < ($4::date + 1)::timestamp)
                OR (
                    $4::date IS NULL
                    AND ($5::date IS NULL OR start_time >= $5::date::timestamp)
                    AND ($6::date IS NULL OR start_time < ($6::date + 1)::timestamp)
                )
            )
        "#,
    )
    .bind(query.content_config_id)
    .bind(trim_optional_string(query.anchor_uid.as_deref()))
    .bind(trim_optional_string(query.anchor_name.as_deref()))
    .bind(query.date)
    .bind(query.date_from)
    .bind(query.date_to)
    .fetch_one(pool)
    .await?;

    Ok(LiveSummaryDto {
        live_exposure_pv: row.try_get("live_exposure_pv")?,
        avg_acu: row.try_get("avg_acu")?,
        live_session_count: row.try_get("live_session_count")?,
        anchor_count: row.try_get("anchor_count")?,
    })
}

pub async fn top_video_growth(
    pool: &PgPool,
    query: VideoGrowthQuery,
) -> anyhow::Result<Vec<VideoGrowthDto>> {
    let rows = sqlx::query(
        r#"
        WITH series AS (
            SELECT
                vc.content_config_id,
                vc.video_id,
                vc.publish_time,
                vc.author_name,
                vc.author_uid,
                vc.title,
                vc.audit_result,
                vc.label,
                vc.audit_extra,
                m.stat_date,
                m.play_count,
                m.imported_at,
                LAG(m.play_count) OVER (
                    PARTITION BY m.content_config_id, m.video_id
                    ORDER BY m.stat_date
                ) AS prev_play_count
            FROM video_content vc
            JOIN video_daily_metric m
                ON m.content_config_id = vc.content_config_id
                AND m.video_id = vc.video_id
            WHERE
                ($1::bigint IS NULL OR vc.content_config_id = $1)
                AND ($2::text IS NULL OR vc.author_uid = $2)
                AND ($3::text IS NULL OR vc.author_name ILIKE '%' || $3 || '%')
                AND ($4::text IS NULL OR lower(NULLIF(btrim(vc.label), '')) = lower($4))
        ),
        filtered AS (
            SELECT
                *,
                CASE
                    WHEN play_count IS NULL THEN NULL
                    ELSE GREATEST(play_count - COALESCE(prev_play_count, 0), 0)
                END AS play_increment
            FROM series
            WHERE
                (
                    ($5::date IS NOT NULL AND stat_date = $5)
                    OR (
                        $5::date IS NULL
                        AND ($6::date IS NULL OR stat_date >= $6)
                        AND ($7::date IS NULL OR stat_date <= $7)
                    )
                )
        )
        SELECT
            content_config_id,
            video_id,
            publish_time,
            author_name,
            author_uid,
            title,
            audit_result,
            label,
            audit_extra,
            COALESCE(SUM(COALESCE(play_increment, 0)), 0)::bigint AS play_growth,
            AVG(COALESCE(play_increment, 0))::float8 AS avg_daily_play_growth,
            COUNT(*)::bigint AS metric_days,
            (ARRAY_AGG(play_count ORDER BY stat_date DESC, imported_at DESC))[1] AS latest_play_count,
            MAX(stat_date) AS latest_stat_date
        FROM filtered
        GROUP BY
            content_config_id,
            video_id,
            publish_time,
            author_name,
            author_uid,
            title,
            audit_result,
            label,
            audit_extra
        ORDER BY play_growth DESC, avg_daily_play_growth DESC, latest_stat_date DESC
        LIMIT $8
        "#,
    )
    .bind(query.content_config_id)
    .bind(trim_optional_string(query.author_uid.as_deref()))
    .bind(trim_optional_string(query.author_name.as_deref()))
    .bind(trim_optional_string(query.label.as_deref()))
    .bind(query.date)
    .bind(query.date_from)
    .bind(query.date_to)
    .bind(query.limit())
    .fetch_all(pool)
    .await?;

    rows.into_iter()
        .map(|row| {
            Ok(VideoGrowthDto {
                content_config_id: row.try_get("content_config_id")?,
                video_id: row.try_get("video_id")?,
                publish_time: row.try_get("publish_time")?,
                author_name: row.try_get("author_name")?,
                author_uid: row.try_get("author_uid")?,
                title: row.try_get("title")?,
                audit_result: row.try_get("audit_result")?,
                label: row.try_get("label")?,
                audit_extra: crate::server::audit_extra_query::redact_audit_extra(
                    row.try_get("audit_extra")?,
                ),
                play_growth: row.try_get("play_growth")?,
                avg_daily_play_growth: row.try_get("avg_daily_play_growth")?,
                metric_days: row.try_get("metric_days")?,
                latest_play_count: row.try_get("latest_play_count")?,
                latest_stat_date: row.try_get("latest_stat_date")?,
            })
        })
        .collect()
}

pub async fn list_video_contents_v2(
    pool: &PgPool,
    query: OperationalContentQuery,
) -> anyhow::Result<PagedResponse<VideoContentDto>> {
    let offset = query.resolved_offset()?;
    let search = trim_optional_string(query.search.as_deref());
    let total: i64 = sqlx::query_scalar(
        r#"
        SELECT COUNT(*)
        FROM video_content v
        JOIN xingtu_activity_content_config c ON c.content_config_id = v.content_config_id
        WHERE ($1::bigint IS NULL OR c.activity_period_id = $1)
            AND ($2::bigint IS NULL OR v.content_config_id = $2)
            AND ($3::text IS NULL OR v.audit_result = $3)
            AND ($4::text IS NULL OR lower(NULLIF(btrim(v.label), '')) = lower($4))
            AND ($5::date IS NULL OR v.publish_time >= $5::date::timestamp)
            AND ($6::date IS NULL OR v.publish_time < ($6::date + 1)::timestamp)
            AND ($7::text IS NULL OR v.video_id ILIKE '%' || $7 || '%'
                OR v.title ILIKE '%' || $7 || '%' OR v.author_name ILIKE '%' || $7 || '%'
                OR v.label ILIKE '%' || $7 || '%')
        "#,
    )
    .bind(query.activity_period_id)
    .bind(query.content_config_id)
    .bind(trim_optional_string(query.status.as_deref()))
    .bind(trim_optional_string(query.label.as_deref()))
    .bind(query.date_from)
    .bind(query.date_to)
    .bind(&search)
    .fetch_one(pool)
    .await?;
    let rows = sqlx::query(
        r#"
        SELECT v.content_config_id, v.video_id, v.publish_time, v.author_name, v.author_uid,
            v.title, v.audit_result, v.label, v.audit_extra, v.last_seen_at
        FROM video_content v
        JOIN xingtu_activity_content_config c ON c.content_config_id = v.content_config_id
        WHERE ($1::bigint IS NULL OR c.activity_period_id = $1)
            AND ($2::bigint IS NULL OR v.content_config_id = $2)
            AND ($3::text IS NULL OR v.audit_result = $3)
            AND ($4::text IS NULL OR lower(NULLIF(btrim(v.label), '')) = lower($4))
            AND ($5::date IS NULL OR v.publish_time >= $5::date::timestamp)
            AND ($6::date IS NULL OR v.publish_time < ($6::date + 1)::timestamp)
            AND ($7::text IS NULL OR v.video_id ILIKE '%' || $7 || '%'
                OR v.title ILIKE '%' || $7 || '%' OR v.author_name ILIKE '%' || $7 || '%'
                OR v.label ILIKE '%' || $7 || '%')
        ORDER BY v.last_seen_at DESC, v.content_config_id, v.video_id
        LIMIT $8 OFFSET $9
        "#,
    )
    .bind(query.activity_period_id)
    .bind(query.content_config_id)
    .bind(trim_optional_string(query.status.as_deref()))
    .bind(trim_optional_string(query.label.as_deref()))
    .bind(query.date_from)
    .bind(query.date_to)
    .bind(search)
    .bind(query.limit())
    .bind(offset)
    .fetch_all(pool)
    .await?;
    let data = rows
        .into_iter()
        .map(|row| {
            Ok(VideoContentDto {
                content_config_id: row.try_get("content_config_id")?,
                video_id: row.try_get("video_id")?,
                publish_time: row.try_get("publish_time")?,
                author_name: row.try_get("author_name")?,
                author_uid: row.try_get("author_uid")?,
                title: row.try_get("title")?,
                audit_result: row.try_get("audit_result")?,
                label: row.try_get("label")?,
                audit_extra: crate::server::audit_extra_query::redact_audit_extra(
                    row.try_get("audit_extra")?,
                ),
                last_seen_at: row.try_get("last_seen_at")?,
            })
        })
        .collect::<Result<Vec<_>, sqlx::Error>>()?;
    Ok(paged(data, total, offset))
}

pub async fn list_live_sessions_v2(
    pool: &PgPool,
    query: OperationalContentQuery,
) -> anyhow::Result<PagedResponse<LiveSessionDto>> {
    let offset = query.resolved_offset()?;
    let search = trim_optional_string(query.search.as_deref());
    let total: i64 = sqlx::query_scalar(
        r#"
        SELECT COUNT(*)
        FROM live_session l
        JOIN xingtu_activity_content_config c ON c.content_config_id = l.content_config_id
        WHERE ($1::bigint IS NULL OR c.activity_period_id = $1)
            AND ($2::bigint IS NULL OR l.content_config_id = $2)
            AND ($3::text IS NULL OR l.audit_result = $3)
            AND ($4::date IS NULL OR l.start_time >= $4::date::timestamp)
            AND ($5::date IS NULL OR l.start_time < ($5::date + 1)::timestamp)
            AND ($6::text IS NULL OR l.live_room_id ILIKE '%' || $6 || '%'
                OR l.title ILIKE '%' || $6 || '%' OR l.anchor_name ILIKE '%' || $6 || '%')
        "#,
    )
    .bind(query.activity_period_id)
    .bind(query.content_config_id)
    .bind(trim_optional_string(query.status.as_deref()))
    .bind(query.date_from)
    .bind(query.date_to)
    .bind(&search)
    .fetch_one(pool)
    .await?;
    let rows = sqlx::query(
        r#"
        SELECT l.content_config_id, l.live_room_id, l.start_time, l.anchor_name, l.anchor_uid,
            l.title, l.cumulative_viewer_count, l.live_exposure_pv,
            l.acu::float8 AS acu, l.audit_result, l.audit_extra, l.last_seen_at
        FROM live_session l
        JOIN xingtu_activity_content_config c ON c.content_config_id = l.content_config_id
        WHERE ($1::bigint IS NULL OR c.activity_period_id = $1)
            AND ($2::bigint IS NULL OR l.content_config_id = $2)
            AND ($3::text IS NULL OR l.audit_result = $3)
            AND ($4::date IS NULL OR l.start_time >= $4::date::timestamp)
            AND ($5::date IS NULL OR l.start_time < ($5::date + 1)::timestamp)
            AND ($6::text IS NULL OR l.live_room_id ILIKE '%' || $6 || '%'
                OR l.title ILIKE '%' || $6 || '%' OR l.anchor_name ILIKE '%' || $6 || '%')
        ORDER BY l.last_seen_at DESC, l.content_config_id, l.live_room_id
        LIMIT $7 OFFSET $8
        "#,
    )
    .bind(query.activity_period_id)
    .bind(query.content_config_id)
    .bind(trim_optional_string(query.status.as_deref()))
    .bind(query.date_from)
    .bind(query.date_to)
    .bind(search)
    .bind(query.limit())
    .bind(offset)
    .fetch_all(pool)
    .await?;
    let data = rows
        .into_iter()
        .map(|row| {
            Ok(LiveSessionDto {
                content_config_id: row.try_get("content_config_id")?,
                live_room_id: row.try_get("live_room_id")?,
                start_time: row.try_get("start_time")?,
                anchor_name: row.try_get("anchor_name")?,
                anchor_uid: row.try_get("anchor_uid")?,
                title: row.try_get("title")?,
                cumulative_viewer_count: row.try_get("cumulative_viewer_count")?,
                live_exposure_pv: row.try_get("live_exposure_pv")?,
                acu: row.try_get("acu")?,
                audit_result: row.try_get("audit_result")?,
                audit_extra: crate::server::audit_extra_query::redact_audit_extra(
                    row.try_get("audit_extra")?,
                ),
                last_seen_at: row.try_get("last_seen_at")?,
            })
        })
        .collect::<Result<Vec<_>, sqlx::Error>>()?;
    Ok(paged(data, total, offset))
}

pub async fn list_feishu_sources_v2(
    pool: &PgPool,
    query: OperationalContentQuery,
) -> anyhow::Result<PagedResponse<FeishuSourceDto>> {
    let offset = query.resolved_offset()?;
    let search = trim_optional_string(query.search.as_deref());
    let total: i64 = sqlx::query_scalar(
        r#"
        SELECT COUNT(*)
        FROM xingtu_feishu_source s
        JOIN xingtu_activity_content_config c ON c.content_config_id = s.content_config_id
        JOIN xingtu_activity_period a ON a.activity_period_id = c.activity_period_id
        JOIN xingtu_project project ON project.project_id = a.project_id
        WHERE ($1::bigint IS NULL OR c.activity_period_id = $1)
            AND ($2::bigint IS NULL OR s.content_config_id = $2)
            AND ($3::text IS NULL OR s.import_status::text = $3)
            AND ($4::date IS NULL OR s.stat_date >= $4)
            AND ($5::date IS NULL OR s.stat_date <= $5)
            AND ($6::text IS NULL OR s.feishu_sheet_url ILIKE '%' || $6 || '%'
                OR project.project_key ILIKE '%' || $6 || '%'
                OR project.display_name ILIKE '%' || $6 || '%' OR a.period ILIKE '%' || $6 || '%')
        "#,
    )
    .bind(query.activity_period_id)
    .bind(query.content_config_id)
    .bind(trim_optional_string(query.status.as_deref()))
    .bind(query.date_from)
    .bind(query.date_to)
    .bind(&search)
    .fetch_one(pool)
    .await?;
    let rows = sqlx::query(
        r#"
        SELECT s.feishu_source_id, s.content_config_id, s.content_type::text AS content_type,
            s.feishu_sheet_url, s.trigger_type::text AS trigger_type, s.stat_date,
            s.pulled_at, s.is_daily_final, s.import_status::text AS import_status,
            s.imported_row_count
        FROM xingtu_feishu_source s
        JOIN xingtu_activity_content_config c ON c.content_config_id = s.content_config_id
        JOIN xingtu_activity_period a ON a.activity_period_id = c.activity_period_id
        JOIN xingtu_project project ON project.project_id = a.project_id
        WHERE ($1::bigint IS NULL OR c.activity_period_id = $1)
            AND ($2::bigint IS NULL OR s.content_config_id = $2)
            AND ($3::text IS NULL OR s.import_status::text = $3)
            AND ($4::date IS NULL OR s.stat_date >= $4)
            AND ($5::date IS NULL OR s.stat_date <= $5)
            AND ($6::text IS NULL OR s.feishu_sheet_url ILIKE '%' || $6 || '%'
                OR project.project_key ILIKE '%' || $6 || '%'
                OR project.display_name ILIKE '%' || $6 || '%' OR a.period ILIKE '%' || $6 || '%')
        ORDER BY s.pulled_at DESC, s.feishu_source_id DESC
        LIMIT $7 OFFSET $8
        "#,
    )
    .bind(query.activity_period_id)
    .bind(query.content_config_id)
    .bind(trim_optional_string(query.status.as_deref()))
    .bind(query.date_from)
    .bind(query.date_to)
    .bind(search)
    .bind(query.limit())
    .bind(offset)
    .fetch_all(pool)
    .await?;
    let data = rows
        .into_iter()
        .map(|row| {
            Ok(FeishuSourceDto {
                feishu_source_id: row.try_get("feishu_source_id")?,
                content_config_id: row.try_get("content_config_id")?,
                content_type: row.try_get("content_type")?,
                feishu_sheet_url: row.try_get("feishu_sheet_url")?,
                trigger_type: row.try_get("trigger_type")?,
                stat_date: row.try_get("stat_date")?,
                pulled_at: row.try_get("pulled_at")?,
                is_daily_final: row.try_get("is_daily_final")?,
                import_status: row.try_get("import_status")?,
                imported_row_count: row.try_get("imported_row_count")?,
            })
        })
        .collect::<Result<Vec<_>, sqlx::Error>>()?;
    Ok(paged(data, total, offset))
}

fn paged<T>(data: Vec<T>, total: i64, offset: i64) -> PagedResponse<T> {
    let next_offset = offset.saturating_add(i64::try_from(data.len()).unwrap_or(i64::MAX));
    let has_more = next_offset < total;
    PagedResponse {
        ok: true,
        data,
        meta: PageMeta {
            total,
            has_more,
            next_cursor: has_more.then(|| URL_SAFE_NO_PAD.encode(next_offset.to_string())),
        },
    }
}

fn trim_optional_string(value: Option<&str>) -> Option<String> {
    value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

fn validate_optional_date_range(
    date_from: Option<NaiveDate>,
    date_to: Option<NaiveDate>,
) -> anyhow::Result<()> {
    if let (Some(date_from), Some(date_to)) = (date_from, date_to) {
        validate_required_date_range(date_from, date_to)?;
    } else if let Some(date_to) = date_to {
        anyhow::ensure!(date_to.succ_opt().is_some(), "date_to 超出可查询范围");
    }
    Ok(())
}

fn validate_required_date_range(date_from: NaiveDate, date_to: NaiveDate) -> anyhow::Result<()> {
    anyhow::ensure!(date_from <= date_to, "date_from 不能晚于 date_to");
    anyhow::ensure!(date_to.succ_opt().is_some(), "date_to 超出可查询范围");
    Ok(())
}

fn beijing_report_range(date_from: NaiveDate, date_to: NaiveDate) -> BeijingReportRangeDto {
    BeijingReportRangeDto {
        timezone: "Asia/Shanghai".to_owned(),
        date_from,
        date_to,
        start_at: date_from
            .and_hms_opt(0, 0, 0)
            .expect("有效日期必定存在当天零点"),
        end_at: date_to
            .and_hms_opt(23, 59, 59)
            .expect("有效日期必定存在当天最后一秒"),
    }
}

#[cfg(test)]
mod pagination_tests {
    use super::*;

    #[test]
    fn cursor_round_trip_and_validation_are_stable() {
        let page = paged(vec!["a", "b"], 10, 5);
        assert!(page.meta.has_more);
        let cursor = page.meta.next_cursor.unwrap();
        let query = OperationalContentQuery {
            cursor: Some(cursor),
            offset: Some(999),
            ..OperationalContentQuery::default()
        };
        assert_eq!(query.resolved_offset().unwrap(), 7);

        let invalid = OperationalContentQuery {
            cursor: Some("not-a-canonical-cursor".to_owned()),
            ..OperationalContentQuery::default()
        };
        assert!(invalid.resolved_offset().is_err());
    }

    #[test]
    fn date_range_is_inclusive_for_beijing_business_days() {
        let date_from = NaiveDate::from_ymd_opt(2026, 8, 3).unwrap();
        let date_to = NaiveDate::from_ymd_opt(2026, 8, 9).unwrap();
        let range = beijing_report_range(date_from, date_to);
        assert_eq!(range.timezone, "Asia/Shanghai");
        assert_eq!(range.start_at.to_string(), "2026-08-03 00:00:00");
        assert_eq!(range.end_at.to_string(), "2026-08-09 23:59:59");
        assert!(validate_required_date_range(date_from, date_to).is_ok());
        assert!(validate_required_date_range(date_to, date_from).is_err());
    }

    #[test]
    fn weekly_label_summary_limits_pathological_ranges() {
        let query = VideoLabelSummaryQuery {
            activity_period_id: None,
            content_config_id: None,
            status: None,
            label: None,
            date_from: NaiveDate::from_ymd_opt(2026, 1, 1).unwrap(),
            date_to: NaiveDate::from_ymd_opt(2027, 1, 2).unwrap(),
        };
        assert!(query.validate().is_err());
    }
}
