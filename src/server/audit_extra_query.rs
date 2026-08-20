use chrono::{DateTime, NaiveDate, NaiveDateTime, Utc};
use salvo::oapi::ToSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sqlx::{PgPool, Row, types::Json};
use std::collections::BTreeMap;

const DEFAULT_PAGE_LIMIT: i64 = 100;
const MAX_PAGE_LIMIT: i64 = 500;
const MAX_CONDITIONS: usize = 20;
const MAX_KEY_LENGTH: usize = 100;
const SENSITIVE_AUDIT_EXTRA_KEY: &str = "key";

/// audit_extra 精确匹配查询请求。
#[derive(Debug, Clone, Deserialize, Serialize, ToSchema)]
pub struct AuditExtraSearchRequest {
    /// 主项目 ID，必须与 activity_period_id 的所属项目一致。
    pub project_id: i64,
    /// 活动期次 ID。历史期次也允许查询，不要求当前处于启用状态。
    pub activity_period_id: i64,
    /// audit_extra 的键值条件。至少一个、最多 20 个；多个条件使用 AND。
    /// 值保留 JSON 类型，数字 1、字符串 "1" 和布尔值 true 是不同条件。
    /// 顶层字段名 `key` 可以参与筛选，但属于机密项，绝不会在响应中回显。
    pub conditions: BTreeMap<String, Value>,
    /// 审核结果精确匹配；不传时同时返回所有审核结果。常见值为“审核通过”“不通过”。
    pub audit_result: Option<String>,
    /// 视频和直播各自最多返回多少条，默认 100，范围 1..500。
    pub limit: Option<i64>,
    /// 视频和直播各自的分页偏移量，默认 0。
    pub offset: Option<i64>,
}

impl AuditExtraSearchRequest {
    pub fn normalized(mut self) -> anyhow::Result<Self> {
        validate_scope_and_conditions(
            self.project_id,
            self.activity_period_id,
            &self.conditions,
            true,
        )?;
        normalize_audit_result(&mut self.audit_result);
        self.limit = Some(self.limit());
        self.offset = Some(self.offset());
        Ok(self)
    }

    fn limit(&self) -> i64 {
        self.limit
            .unwrap_or(DEFAULT_PAGE_LIMIT)
            .clamp(1, MAX_PAGE_LIMIT)
    }

    fn offset(&self) -> i64 {
        self.offset.unwrap_or(0).max(0)
    }

    fn conditions_json(&self) -> Value {
        conditions_json(&self.conditions)
    }

    /// 复杂 JSON 条件仍使用 GIN 做候选预筛；字符串、数字、布尔和 null
    /// 需要兼容历史飞书 `{ type, value }` 包装，因此只做逻辑值比较。
    fn indexable_conditions_json(&self) -> Value {
        indexable_conditions_json(&self.conditions)
    }

    /// 敏感条件不进入查询缓存，避免条件值出现在内存缓存键中。
    pub fn contains_sensitive_condition(&self) -> bool {
        contains_sensitive_condition(&self.conditions)
    }
}

fn validate_scope_and_conditions(
    project_id: i64,
    activity_period_id: i64,
    conditions: &BTreeMap<String, Value>,
    require_condition: bool,
) -> anyhow::Result<()> {
    anyhow::ensure!(project_id > 0, "project_id 必须是正整数");
    anyhow::ensure!(activity_period_id > 0, "activity_period_id 必须是正整数");
    if require_condition {
        anyhow::ensure!(!conditions.is_empty(), "conditions 至少需要一个键值条件");
    }
    anyhow::ensure!(
        conditions.len() <= MAX_CONDITIONS,
        "conditions 最多允许 {MAX_CONDITIONS} 个键值条件"
    );
    for key in conditions.keys() {
        anyhow::ensure!(!key.trim().is_empty(), "conditions 中的 key 不能为空");
        anyhow::ensure!(key == key.trim(), "conditions 中的 key 不能包含首尾空白");
        anyhow::ensure!(
            key.chars().count() <= MAX_KEY_LENGTH,
            "conditions 中的 key 不能超过 {MAX_KEY_LENGTH} 个字符"
        );
    }
    Ok(())
}

fn normalize_audit_result(audit_result: &mut Option<String>) {
    *audit_result = audit_result
        .take()
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty());
}

fn conditions_json(conditions: &BTreeMap<String, Value>) -> Value {
    json!(conditions)
}

fn indexable_conditions_json(conditions: &BTreeMap<String, Value>) -> Value {
    json!(
        conditions
            .iter()
            .filter(|(_, value)| value.is_array() || value.is_object())
            .collect::<BTreeMap<_, _>>()
    )
}

fn contains_sensitive_condition(conditions: &BTreeMap<String, Value>) -> bool {
    conditions.contains_key(SENSITIVE_AUDIT_EXTRA_KEY)
}

/// 实际命中的项目和期次。
#[derive(Debug, Clone, Deserialize, Serialize, ToSchema)]
pub struct AuditExtraScopeDto {
    pub project_id: i64,
    pub project_key: String,
    pub project_display_name: String,
    pub activity_period_id: i64,
    pub period: String,
    pub period_code: Option<String>,
}

/// 响应中回显的标准化过滤条件。
#[derive(Debug, Clone, Deserialize, Serialize, ToSchema)]
pub struct AuditExtraFiltersDto {
    /// 所有键值均按 JSON 类型精确匹配，多个键值之间为 AND。
    /// 请求中的顶层 `key` 机密条件不会在这里回显。
    pub conditions: BTreeMap<String, Value>,
    /// 审核结果精确匹配值；null 表示不过滤。
    pub audit_result: Option<String>,
}

/// audit_extra 查询的全量聚合，不受 limit/offset 影响。
#[derive(Debug, Clone, Deserialize, Serialize, ToSchema)]
pub struct AuditExtraSummaryDto {
    /// 命中的视频数量。
    pub video_count: i64,
    /// 命中的直播场次数量。
    pub live_session_count: i64,
    /// 视频数量与直播场次数量之和。
    pub total_content_count: i64,
    /// 每个命中视频取最新一个每日指标后的 play_count 之和；无指标按 0。
    pub total_play_count: i64,
    /// 命中视频中至少存在一个每日指标快照的视频数量。
    pub video_with_metric_count: i64,
    /// 命中直播场次当前最新 live_exposure_pv（场观 PV）之和；空值按 0。
    pub total_live_exposure_pv: i64,
}

#[derive(Debug, Clone, Deserialize, Serialize, ToSchema)]
pub struct AuditExtraPageMetaDto {
    /// 全部命中数量，不受本页 limit/offset 影响。
    pub total: i64,
    pub limit: i64,
    pub offset: i64,
    pub has_more: bool,
    pub next_offset: Option<i64>,
}

#[derive(Debug, Clone, Deserialize, Serialize, ToSchema)]
pub struct AuditExtraPageDto<T> {
    pub data: Vec<T>,
    pub meta: AuditExtraPageMetaDto,
}

/// 视频最新一个每日指标快照。
#[derive(Debug, Clone, Deserialize, Serialize, ToSchema)]
pub struct LatestVideoMetricDto {
    pub stat_date: NaiveDate,
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
    pub is_daily_final: bool,
    pub pulled_at: Option<DateTime<Utc>>,
    pub imported_at: DateTime<Utc>,
}

/// 命中 audit_extra 条件的视频详情及最新指标。
#[derive(Debug, Clone, Deserialize, Serialize, ToSchema)]
pub struct AuditExtraVideoDto {
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
    /// 没有任何 video_daily_metric 时为 null。
    pub latest_metric: Option<LatestVideoMetricDto>,
    pub last_seen_at: DateTime<Utc>,
}

/// 命中 audit_extra 条件的直播详情。live_session 本身保存每场直播的最新数据。
#[derive(Debug, Clone, Deserialize, Serialize, ToSchema)]
pub struct AuditExtraLiveSessionDto {
    pub content_config_id: i64,
    pub live_room_id: String,
    pub start_time: NaiveDateTime,
    pub anchor_name: Option<String>,
    pub anchor_uid: Option<String>,
    pub title: Option<String>,
    pub cumulative_viewer_count: Option<i64>,
    /// 业务场观 PV，只使用 live_exposure_pv。
    pub live_exposure_pv: Option<i64>,
    pub exposure_uv: Option<i64>,
    pub acu: Option<f64>,
    pub pcu: Option<i64>,
    pub comment_count: Option<i64>,
    pub share_count: Option<i64>,
    pub component_click_count: Option<i64>,
    pub android_download_or_activate_count: Option<i64>,
    pub ios_download_or_activate_count: Option<i64>,
    pub live_duration_seconds: Option<i64>,
    pub avg_watch_duration_seconds: Option<i64>,
    pub follower_increase_count: Option<i64>,
    pub like_rate: Option<f64>,
    pub comment_rate: Option<f64>,
    pub live_game_name: Option<String>,
    pub pulled_at: Option<DateTime<Utc>>,
    pub audit_result: Option<String>,
    pub label: Option<String>,
    /// 审核扩展字段；顶层机密字段 `key` 永不返回。
    pub audit_extra: Value,
    pub last_seen_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Deserialize, Serialize, ToSchema)]
pub struct AuditExtraSearchResponse {
    pub ok: bool,
    pub scope: AuditExtraScopeDto,
    pub filters: AuditExtraFiltersDto,
    pub summary: AuditExtraSummaryDto,
    /// 视频与直播独立分页，使用相同的 limit/offset。
    pub videos: AuditExtraPageDto<AuditExtraVideoDto>,
    /// 视频与直播独立分页，使用相同的 limit/offset。
    pub live_sessions: AuditExtraPageDto<AuditExtraLiveSessionDto>,
}

/// 按 UID 加权平均 ACU 门槛汇总直播 PV 的请求。
#[derive(Debug, Clone, Deserialize, Serialize, ToSchema)]
pub struct WeightedAcuLivePvRequest {
    /// 主项目 ID，必须与 activity_period_id 的所属项目一致。
    pub project_id: i64,
    /// 活动期次 ID。历史期次也允许查询。
    pub activity_period_id: i64,
    /// 可选 audit_extra 精确条件，多个条件使用 AND；空对象表示不过滤 audit_extra。
    /// 顶层字段名 `key` 可以参与筛选，但不会在响应或缓存中出现。
    #[serde(default)]
    pub conditions: BTreeMap<String, Value>,
    /// 可选审核结果精确匹配，如“审核通过”或“不通过”。
    pub audit_result: Option<String>,
    /// 严格小于该值的 UID 会被选中。例如 10 表示 weighted_average_acu < 10。
    pub weighted_average_acu_lt: f64,
}

impl WeightedAcuLivePvRequest {
    pub fn normalized(mut self) -> anyhow::Result<Self> {
        validate_scope_and_conditions(
            self.project_id,
            self.activity_period_id,
            &self.conditions,
            false,
        )?;
        anyhow::ensure!(
            self.weighted_average_acu_lt.is_finite(),
            "weighted_average_acu_lt 必须是有限数字"
        );
        anyhow::ensure!(
            self.weighted_average_acu_lt >= 0.0,
            "weighted_average_acu_lt 不能小于 0"
        );
        normalize_audit_result(&mut self.audit_result);
        Ok(self)
    }

    pub fn contains_sensitive_condition(&self) -> bool {
        contains_sensitive_condition(&self.conditions)
    }
}

#[derive(Debug, Clone, Deserialize, Serialize, ToSchema)]
pub struct WeightedAcuLivePvFiltersDto {
    /// 已标准化的公开 audit_extra 条件；顶层机密字段 `key` 不回显。
    pub conditions: BTreeMap<String, Value>,
    pub audit_result: Option<String>,
    /// UID 加权平均 ACU 的严格上界。
    pub weighted_average_acu_lt: f64,
}

#[derive(Debug, Clone, Deserialize, Serialize, ToSchema)]
pub struct WeightedAcuLivePvSummaryDto {
    /// audit_extra、审核结果和期次范围内，具有非空 anchor_uid 的 UID 数量。
    pub candidate_user_count: i64,
    /// 至少有一场同时具备 ACU 且直播时长大于 0，可计算加权平均 ACU 的 UID 数量。
    pub evaluated_user_count: i64,
    /// 加权平均 ACU 严格低于门槛的 UID 数量。
    pub selected_user_count: i64,
    /// 所有选中 UID 在筛选范围内的直播场次数；包括该 UID 缺少 ACU/有效时长的其他场次。
    pub selected_live_session_count: i64,
    /// 所有选中 UID 在筛选范围内的 live_exposure_pv 总和；空值按 0。
    pub total_live_exposure_pv: i64,
}

#[derive(Debug, Clone, Deserialize, Serialize, ToSchema)]
pub struct WeightedAcuLivePvResponse {
    pub ok: bool,
    pub scope: AuditExtraScopeDto,
    pub filters: WeightedAcuLivePvFiltersDto,
    pub summary: WeightedAcuLivePvSummaryDto,
}

/// 按 UID 视频总播放门槛汇总播放量的请求。
#[derive(Debug, Clone, Deserialize, Serialize, ToSchema)]
pub struct AuthorPlayVideoTotalRequest {
    /// 主项目 ID，必须与 activity_period_id 的所属项目一致。
    pub project_id: i64,
    /// 活动期次 ID。历史期次也允许查询。
    pub activity_period_id: i64,
    /// 可选 audit_extra 精确条件，多个条件使用 AND；空对象表示不过滤 audit_extra。
    /// 顶层字段名 `key` 可以参与筛选，但不会在响应或缓存中出现。
    #[serde(default)]
    pub conditions: BTreeMap<String, Value>,
    /// 可选审核结果精确匹配，如“审核通过”或“不通过”。
    pub audit_result: Option<String>,
    /// 严格小于该值的 UID 会被选中。例如 100000 表示 author_total_play_count < 100000。
    pub author_total_play_count_lt: i64,
}

impl AuthorPlayVideoTotalRequest {
    pub fn normalized(mut self) -> anyhow::Result<Self> {
        validate_scope_and_conditions(
            self.project_id,
            self.activity_period_id,
            &self.conditions,
            false,
        )?;
        anyhow::ensure!(
            self.author_total_play_count_lt >= 0,
            "author_total_play_count_lt 不能小于 0"
        );
        normalize_audit_result(&mut self.audit_result);
        Ok(self)
    }

    pub fn contains_sensitive_condition(&self) -> bool {
        contains_sensitive_condition(&self.conditions)
    }
}

#[derive(Debug, Clone, Deserialize, Serialize, ToSchema)]
pub struct AuthorPlayVideoTotalFiltersDto {
    /// 已标准化的公开 audit_extra 条件；顶层机密字段 `key` 不回显。
    pub conditions: BTreeMap<String, Value>,
    pub audit_result: Option<String>,
    /// UID 视频总播放的严格上界。
    pub author_total_play_count_lt: i64,
}

#[derive(Debug, Clone, Deserialize, Serialize, ToSchema)]
pub struct AuthorPlayVideoTotalSummaryDto {
    /// audit_extra、审核结果和期次范围内，具有非空 author_uid 的 UID 数量。
    pub candidate_user_count: i64,
    /// 视频最新播放量合计严格低于门槛的 UID 数量。
    pub selected_user_count: i64,
    /// 所有选中 UID 在筛选范围内的视频数量。
    pub selected_video_count: i64,
    /// 选中视频中至少存在一个 video_daily_metric 快照的视频数量。
    pub selected_video_with_metric_count: i64,
    /// 所有选中 UID 的视频最新 play_count 总和；无指标或空值按 0。
    pub total_play_count: i64,
}

#[derive(Debug, Clone, Deserialize, Serialize, ToSchema)]
pub struct AuthorPlayVideoTotalResponse {
    pub ok: bool,
    pub scope: AuditExtraScopeDto,
    pub filters: AuthorPlayVideoTotalFiltersDto,
    pub summary: AuthorPlayVideoTotalSummaryDto,
}

/// 按项目、期次和一个或多个 audit_extra 键值精确查询视频与直播。
pub async fn search(
    pool: &PgPool,
    request: AuditExtraSearchRequest,
) -> anyhow::Result<Option<AuditExtraSearchResponse>> {
    let request = request.normalized()?;
    let limit = request.limit();
    let offset = request.offset();
    let audit_result = request.audit_result.clone();
    let conditions = Json(request.conditions_json());
    let indexable_conditions = Json(request.indexable_conditions_json());
    let mut tx = pool.begin().await?;
    sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ, READ ONLY")
        .execute(&mut *tx)
        .await?;

    let scope_row = sqlx::query(
        r#"
        SELECT
            project.project_id,
            project.project_key,
            project.display_name AS project_display_name,
            period.activity_period_id,
            period.period,
            period.period_code
        FROM xingtu_project project
        JOIN xingtu_activity_period period ON period.project_id = project.project_id
        WHERE project.project_id = $1 AND period.activity_period_id = $2
        "#,
    )
    .bind(request.project_id)
    .bind(request.activity_period_id)
    .fetch_optional(&mut *tx)
    .await?;
    let Some(scope_row) = scope_row else {
        tx.rollback().await?;
        return Ok(None);
    };
    let scope = AuditExtraScopeDto {
        project_id: scope_row.try_get("project_id")?,
        project_key: scope_row.try_get("project_key")?,
        project_display_name: scope_row.try_get("project_display_name")?,
        activity_period_id: scope_row.try_get("activity_period_id")?,
        period: scope_row.try_get("period")?,
        period_code: scope_row.try_get("period_code")?,
    };

    let summary_row = sqlx::query(
        r#"
        WITH matched_videos AS (
            SELECT video.content_config_id, video.video_id
            FROM video_content video
            JOIN xingtu_activity_content_config config
                ON config.content_config_id = video.content_config_id
            JOIN xingtu_activity_period period
                ON period.activity_period_id = config.activity_period_id
            WHERE period.project_id = $1
                AND period.activity_period_id = $2
                AND config.content_type = 'video'
                AND video.audit_extra @> $5::jsonb
                AND NOT EXISTS (
                    SELECT 1
                    FROM jsonb_each($3::jsonb) AS condition(key, value)
                    WHERE normalize_audit_extra_field_value(
                        video.audit_extra -> condition.key
                    ) IS DISTINCT FROM condition.value
                )
                AND ($4::text IS NULL OR video.audit_result = $4)
        ),
        latest_video_metrics AS (
            SELECT matched.content_config_id, matched.video_id, metric.stat_date, metric.play_count
            FROM matched_videos matched
            LEFT JOIN LATERAL (
                SELECT daily.stat_date, daily.play_count
                FROM video_daily_metric daily
                WHERE daily.content_config_id = matched.content_config_id
                    AND daily.video_id = matched.video_id
                ORDER BY daily.stat_date DESC, daily.imported_at DESC
                LIMIT 1
            ) metric ON true
        ),
        matched_lives AS (
            SELECT live.live_exposure_pv
            FROM live_session live
            JOIN xingtu_activity_content_config config
                ON config.content_config_id = live.content_config_id
            JOIN xingtu_activity_period period
                ON period.activity_period_id = config.activity_period_id
            WHERE period.project_id = $1
                AND period.activity_period_id = $2
                AND config.content_type = 'live'
                AND live.audit_extra @> $5::jsonb
                AND NOT EXISTS (
                    SELECT 1
                    FROM jsonb_each($3::jsonb) AS condition(key, value)
                    WHERE normalize_audit_extra_field_value(
                        live.audit_extra -> condition.key
                    ) IS DISTINCT FROM condition.value
                )
                AND ($4::text IS NULL OR live.audit_result = $4)
        )
        SELECT
            (SELECT COUNT(*) FROM latest_video_metrics)::bigint AS video_count,
            (SELECT COUNT(stat_date) FROM latest_video_metrics)::bigint
                AS video_with_metric_count,
            (SELECT COALESCE(SUM(COALESCE(play_count, 0)), 0) FROM latest_video_metrics)::bigint
                AS total_play_count,
            (SELECT COUNT(*) FROM matched_lives)::bigint AS live_session_count,
            (SELECT COALESCE(SUM(COALESCE(live_exposure_pv, 0)), 0) FROM matched_lives)::bigint
                AS total_live_exposure_pv
        "#,
    )
    .bind(request.project_id)
    .bind(request.activity_period_id)
    .bind(&conditions)
    .bind(&audit_result)
    .bind(&indexable_conditions)
    .fetch_one(&mut *tx)
    .await?;
    let video_count: i64 = summary_row.try_get("video_count")?;
    let live_session_count: i64 = summary_row.try_get("live_session_count")?;
    let summary = AuditExtraSummaryDto {
        video_count,
        live_session_count,
        total_content_count: video_count.saturating_add(live_session_count),
        total_play_count: summary_row.try_get("total_play_count")?,
        video_with_metric_count: summary_row.try_get("video_with_metric_count")?,
        total_live_exposure_pv: summary_row.try_get("total_live_exposure_pv")?,
    };

    let video_rows = sqlx::query(
        r#"
        SELECT
            video.content_config_id,
            video.video_id,
            video.publish_time,
            video.author_name,
            video.author_uid,
            video.title,
            video.audit_result,
            video.label,
            video.audit_extra,
            video.last_seen_at,
            metric.stat_date,
            metric.play_count,
            metric.valid_play_count,
            metric.like_count,
            metric.valid_like_count,
            metric.comment_count,
            metric.share_count,
            metric.component_click_count,
            metric.android_activate_count,
            metric.ios_activate_count,
            metric.reservation_success_count,
            metric.reservation_install_complete_count,
            metric.follower_increase_count,
            metric.like_rate::float8 AS like_rate,
            metric.comment_rate::float8 AS comment_rate,
            metric.is_daily_final,
            metric.pulled_at,
            metric.imported_at
        FROM video_content video
        JOIN xingtu_activity_content_config config
            ON config.content_config_id = video.content_config_id
        JOIN xingtu_activity_period period
            ON period.activity_period_id = config.activity_period_id
        LEFT JOIN LATERAL (
            SELECT daily.*
            FROM video_daily_metric daily
            WHERE daily.content_config_id = video.content_config_id
                AND daily.video_id = video.video_id
            ORDER BY daily.stat_date DESC, daily.imported_at DESC
            LIMIT 1
        ) metric ON true
        WHERE period.project_id = $1
            AND period.activity_period_id = $2
            AND config.content_type = 'video'
            AND video.audit_extra @> $5::jsonb
            AND NOT EXISTS (
                SELECT 1
                FROM jsonb_each($3::jsonb) AS condition(key, value)
                WHERE normalize_audit_extra_field_value(
                    video.audit_extra -> condition.key
                ) IS DISTINCT FROM condition.value
            )
            AND ($4::text IS NULL OR video.audit_result = $4)
        ORDER BY video.publish_time DESC, video.content_config_id, video.video_id
        LIMIT $6 OFFSET $7
        "#,
    )
    .bind(request.project_id)
    .bind(request.activity_period_id)
    .bind(&conditions)
    .bind(&audit_result)
    .bind(&indexable_conditions)
    .bind(limit)
    .bind(offset)
    .fetch_all(&mut *tx)
    .await?;
    let videos = video_rows
        .into_iter()
        .map(video_from_row)
        .collect::<Result<Vec<_>, sqlx::Error>>()?;

    let live_rows = sqlx::query(
        r#"
        SELECT
            live.content_config_id,
            live.live_room_id,
            live.start_time,
            live.anchor_name,
            live.anchor_uid,
            live.title,
            live.cumulative_viewer_count,
            live.live_exposure_pv,
            live.exposure_uv,
            live.acu::float8 AS acu,
            live.pcu,
            live.comment_count,
            live.share_count,
            live.component_click_count,
            live.android_download_or_activate_count,
            live.ios_download_or_activate_count,
            live.live_duration_seconds,
            live.avg_watch_duration_seconds,
            live.follower_increase_count,
            live.like_rate::float8 AS like_rate,
            live.comment_rate::float8 AS comment_rate,
            live.live_game_name,
            live.pulled_at,
            live.audit_result,
            live.label,
            live.audit_extra,
            live.last_seen_at
        FROM live_session live
        JOIN xingtu_activity_content_config config
            ON config.content_config_id = live.content_config_id
        JOIN xingtu_activity_period period
            ON period.activity_period_id = config.activity_period_id
        WHERE period.project_id = $1
            AND period.activity_period_id = $2
            AND config.content_type = 'live'
            AND live.audit_extra @> $5::jsonb
            AND NOT EXISTS (
                SELECT 1
                FROM jsonb_each($3::jsonb) AS condition(key, value)
                WHERE normalize_audit_extra_field_value(
                    live.audit_extra -> condition.key
                ) IS DISTINCT FROM condition.value
            )
            AND ($4::text IS NULL OR live.audit_result = $4)
        ORDER BY live.start_time DESC, live.content_config_id, live.live_room_id
        LIMIT $6 OFFSET $7
        "#,
    )
    .bind(request.project_id)
    .bind(request.activity_period_id)
    .bind(&conditions)
    .bind(&audit_result)
    .bind(&indexable_conditions)
    .bind(limit)
    .bind(offset)
    .fetch_all(&mut *tx)
    .await?;
    let live_sessions = live_rows
        .into_iter()
        .map(live_from_row)
        .collect::<Result<Vec<_>, sqlx::Error>>()?;

    tx.commit().await?;
    Ok(Some(AuditExtraSearchResponse {
        ok: true,
        scope,
        filters: AuditExtraFiltersDto {
            conditions: redact_condition_map(request.conditions),
            audit_result,
        },
        summary,
        videos: page(videos, video_count, limit, offset),
        live_sessions: page(live_sessions, live_session_count, limit, offset),
    }))
}

/// 在 audit_extra/审核结果筛选范围内，按 anchor_uid 计算时长加权平均 ACU，
/// 选出严格低于门槛的 UID，再汇总这些 UID 的全部直播场观 PV。
pub async fn total_live_pv_below_weighted_acu(
    pool: &PgPool,
    request: WeightedAcuLivePvRequest,
) -> anyhow::Result<Option<WeightedAcuLivePvResponse>> {
    let request = request.normalized()?;
    let audit_result = request.audit_result.clone();
    let conditions = Json(conditions_json(&request.conditions));
    let indexable_conditions = Json(indexable_conditions_json(&request.conditions));
    let mut tx = pool.begin().await?;
    sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ, READ ONLY")
        .execute(&mut *tx)
        .await?;

    let scope_row = sqlx::query(
        r#"
        SELECT
            project.project_id,
            project.project_key,
            project.display_name AS project_display_name,
            period.activity_period_id,
            period.period,
            period.period_code
        FROM xingtu_project project
        JOIN xingtu_activity_period period ON period.project_id = project.project_id
        WHERE project.project_id = $1 AND period.activity_period_id = $2
        "#,
    )
    .bind(request.project_id)
    .bind(request.activity_period_id)
    .fetch_optional(&mut *tx)
    .await?;
    let Some(scope_row) = scope_row else {
        tx.rollback().await?;
        return Ok(None);
    };
    let scope = AuditExtraScopeDto {
        project_id: scope_row.try_get("project_id")?,
        project_key: scope_row.try_get("project_key")?,
        project_display_name: scope_row.try_get("project_display_name")?,
        activity_period_id: scope_row.try_get("activity_period_id")?,
        period: scope_row.try_get("period")?,
        period_code: scope_row.try_get("period_code")?,
    };

    let row = sqlx::query(
        r#"
        WITH candidate_lives AS (
            SELECT
                NULLIF(btrim(live.anchor_uid), '') AS user_uid,
                live.live_exposure_pv,
                live.acu::float8 AS acu,
                live.live_duration_seconds
            FROM live_session live
            JOIN xingtu_activity_content_config config
                ON config.content_config_id = live.content_config_id
            JOIN xingtu_activity_period period
                ON period.activity_period_id = config.activity_period_id
            WHERE period.project_id = $1
                AND period.activity_period_id = $2
                AND config.content_type = 'live'
                AND NULLIF(btrim(live.anchor_uid), '') IS NOT NULL
                AND live.audit_extra @> $5::jsonb
                AND NOT EXISTS (
                    SELECT 1
                    FROM jsonb_each($3::jsonb) AS condition(key, value)
                    WHERE normalize_audit_extra_field_value(
                        live.audit_extra -> condition.key
                    ) IS DISTINCT FROM condition.value
                )
                AND ($4::text IS NULL OR live.audit_result = $4)
        ),
        user_metrics AS (
            SELECT
                user_uid,
                SUM(
                    CASE
                        WHEN acu IS NOT NULL AND live_duration_seconds > 0
                        THEN acu * live_duration_seconds::float8
                    END
                ) / NULLIF(
                    SUM(
                        CASE
                            WHEN acu IS NOT NULL AND live_duration_seconds > 0
                            THEN live_duration_seconds::float8
                        END
                    ),
                    0
                ) AS weighted_average_acu
            FROM candidate_lives
            GROUP BY user_uid
        ),
        selected_users AS (
            SELECT user_uid
            FROM user_metrics
            WHERE weighted_average_acu < $6::float8
        ),
        selected_lives AS (
            SELECT candidate.live_exposure_pv
            FROM candidate_lives candidate
            JOIN selected_users selected USING (user_uid)
        )
        SELECT
            (SELECT COUNT(*) FROM user_metrics)::bigint AS candidate_user_count,
            (SELECT COUNT(weighted_average_acu) FROM user_metrics)::bigint
                AS evaluated_user_count,
            (SELECT COUNT(*) FROM selected_users)::bigint AS selected_user_count,
            (SELECT COUNT(*) FROM selected_lives)::bigint AS selected_live_session_count,
            (
                SELECT COALESCE(SUM(COALESCE(live_exposure_pv, 0)), 0)::bigint
                FROM selected_lives
            ) AS total_live_exposure_pv
        "#,
    )
    .bind(request.project_id)
    .bind(request.activity_period_id)
    .bind(&conditions)
    .bind(&audit_result)
    .bind(&indexable_conditions)
    .bind(request.weighted_average_acu_lt)
    .fetch_one(&mut *tx)
    .await?;

    tx.commit().await?;
    Ok(Some(WeightedAcuLivePvResponse {
        ok: true,
        scope,
        filters: WeightedAcuLivePvFiltersDto {
            conditions: redact_condition_map(request.conditions),
            audit_result,
            weighted_average_acu_lt: request.weighted_average_acu_lt,
        },
        summary: WeightedAcuLivePvSummaryDto {
            candidate_user_count: row.try_get("candidate_user_count")?,
            evaluated_user_count: row.try_get("evaluated_user_count")?,
            selected_user_count: row.try_get("selected_user_count")?,
            selected_live_session_count: row.try_get("selected_live_session_count")?,
            total_live_exposure_pv: row.try_get("total_live_exposure_pv")?,
        },
    }))
}

/// 在 audit_extra/审核结果筛选范围内，每个视频只取最新指标，按 author_uid
/// 汇总播放量，选出严格低于门槛的 UID，再返回这些 UID 的播放量合计。
pub async fn total_video_play_below_author_total(
    pool: &PgPool,
    request: AuthorPlayVideoTotalRequest,
) -> anyhow::Result<Option<AuthorPlayVideoTotalResponse>> {
    let request = request.normalized()?;
    let audit_result = request.audit_result.clone();
    let conditions = Json(conditions_json(&request.conditions));
    let indexable_conditions = Json(indexable_conditions_json(&request.conditions));
    let mut tx = pool.begin().await?;
    sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ, READ ONLY")
        .execute(&mut *tx)
        .await?;

    let scope_row = sqlx::query(
        r#"
        SELECT
            project.project_id,
            project.project_key,
            project.display_name AS project_display_name,
            period.activity_period_id,
            period.period,
            period.period_code
        FROM xingtu_project project
        JOIN xingtu_activity_period period ON period.project_id = project.project_id
        WHERE project.project_id = $1 AND period.activity_period_id = $2
        "#,
    )
    .bind(request.project_id)
    .bind(request.activity_period_id)
    .fetch_optional(&mut *tx)
    .await?;
    let Some(scope_row) = scope_row else {
        tx.rollback().await?;
        return Ok(None);
    };
    let scope = AuditExtraScopeDto {
        project_id: scope_row.try_get("project_id")?,
        project_key: scope_row.try_get("project_key")?,
        project_display_name: scope_row.try_get("project_display_name")?,
        activity_period_id: scope_row.try_get("activity_period_id")?,
        period: scope_row.try_get("period")?,
        period_code: scope_row.try_get("period_code")?,
    };

    let row = sqlx::query(
        r#"
        WITH candidate_videos AS (
            SELECT
                video.content_config_id,
                video.video_id,
                NULLIF(btrim(video.author_uid), '') AS user_uid
            FROM video_content video
            JOIN xingtu_activity_content_config config
                ON config.content_config_id = video.content_config_id
            JOIN xingtu_activity_period period
                ON period.activity_period_id = config.activity_period_id
            WHERE period.project_id = $1
                AND period.activity_period_id = $2
                AND config.content_type = 'video'
                AND NULLIF(btrim(video.author_uid), '') IS NOT NULL
                AND video.audit_extra @> $5::jsonb
                AND NOT EXISTS (
                    SELECT 1
                    FROM jsonb_each($3::jsonb) AS condition(key, value)
                    WHERE normalize_audit_extra_field_value(
                        video.audit_extra -> condition.key
                    ) IS DISTINCT FROM condition.value
                )
                AND ($4::text IS NULL OR video.audit_result = $4)
        ),
        latest_video_metrics AS (
            SELECT
                candidate.user_uid,
                COALESCE(metric.play_count, 0)::bigint AS play_count,
                metric.stat_date IS NOT NULL AS has_metric
            FROM candidate_videos candidate
            LEFT JOIN LATERAL (
                SELECT daily.stat_date, daily.play_count
                FROM video_daily_metric daily
                WHERE daily.content_config_id = candidate.content_config_id
                    AND daily.video_id = candidate.video_id
                ORDER BY daily.stat_date DESC, daily.imported_at DESC
                LIMIT 1
            ) metric ON true
        ),
        user_totals AS (
            SELECT
                user_uid,
                COUNT(*)::bigint AS video_count,
                COUNT(*) FILTER (WHERE has_metric)::bigint AS video_with_metric_count,
                SUM(play_count)::bigint AS total_play_count
            FROM latest_video_metrics
            GROUP BY user_uid
        ),
        selected_users AS (
            SELECT user_uid, video_count, video_with_metric_count, total_play_count
            FROM user_totals
            WHERE total_play_count < $6::bigint
        )
        SELECT
            (SELECT COUNT(*) FROM user_totals)::bigint AS candidate_user_count,
            (SELECT COUNT(*) FROM selected_users)::bigint AS selected_user_count,
            (
                SELECT COALESCE(SUM(video_count), 0)::bigint
                FROM selected_users
            ) AS selected_video_count,
            (
                SELECT COALESCE(SUM(video_with_metric_count), 0)::bigint
                FROM selected_users
            ) AS selected_video_with_metric_count,
            (
                SELECT COALESCE(SUM(total_play_count), 0)::bigint
                FROM selected_users
            ) AS total_play_count
        "#,
    )
    .bind(request.project_id)
    .bind(request.activity_period_id)
    .bind(&conditions)
    .bind(&audit_result)
    .bind(&indexable_conditions)
    .bind(request.author_total_play_count_lt)
    .fetch_one(&mut *tx)
    .await?;

    tx.commit().await?;
    Ok(Some(AuthorPlayVideoTotalResponse {
        ok: true,
        scope,
        filters: AuthorPlayVideoTotalFiltersDto {
            conditions: redact_condition_map(request.conditions),
            audit_result,
            author_total_play_count_lt: request.author_total_play_count_lt,
        },
        summary: AuthorPlayVideoTotalSummaryDto {
            candidate_user_count: row.try_get("candidate_user_count")?,
            selected_user_count: row.try_get("selected_user_count")?,
            selected_video_count: row.try_get("selected_video_count")?,
            selected_video_with_metric_count: row.try_get("selected_video_with_metric_count")?,
            total_play_count: row.try_get("total_play_count")?,
        },
    }))
}

fn video_from_row(row: sqlx::postgres::PgRow) -> Result<AuditExtraVideoDto, sqlx::Error> {
    let stat_date: Option<NaiveDate> = row.try_get("stat_date")?;
    let latest_metric = stat_date
        .map(|stat_date| {
            Ok::<LatestVideoMetricDto, sqlx::Error>(LatestVideoMetricDto {
                stat_date,
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
                is_daily_final: row.try_get("is_daily_final")?,
                pulled_at: row.try_get("pulled_at")?,
                imported_at: row.try_get("imported_at")?,
            })
        })
        .transpose()?;

    Ok(AuditExtraVideoDto {
        content_config_id: row.try_get("content_config_id")?,
        video_id: row.try_get("video_id")?,
        publish_time: row.try_get("publish_time")?,
        author_name: row.try_get("author_name")?,
        author_uid: row.try_get("author_uid")?,
        title: row.try_get("title")?,
        audit_result: row.try_get("audit_result")?,
        label: row.try_get("label")?,
        audit_extra: redact_audit_extra(row.try_get("audit_extra")?),
        latest_metric,
        last_seen_at: row.try_get("last_seen_at")?,
    })
}

fn live_from_row(row: sqlx::postgres::PgRow) -> Result<AuditExtraLiveSessionDto, sqlx::Error> {
    Ok(AuditExtraLiveSessionDto {
        content_config_id: row.try_get("content_config_id")?,
        live_room_id: row.try_get("live_room_id")?,
        start_time: row.try_get("start_time")?,
        anchor_name: row.try_get("anchor_name")?,
        anchor_uid: row.try_get("anchor_uid")?,
        title: row.try_get("title")?,
        cumulative_viewer_count: row.try_get("cumulative_viewer_count")?,
        live_exposure_pv: row.try_get("live_exposure_pv")?,
        exposure_uv: row.try_get("exposure_uv")?,
        acu: row.try_get("acu")?,
        pcu: row.try_get("pcu")?,
        comment_count: row.try_get("comment_count")?,
        share_count: row.try_get("share_count")?,
        component_click_count: row.try_get("component_click_count")?,
        android_download_or_activate_count: row.try_get("android_download_or_activate_count")?,
        ios_download_or_activate_count: row.try_get("ios_download_or_activate_count")?,
        live_duration_seconds: row.try_get("live_duration_seconds")?,
        avg_watch_duration_seconds: row.try_get("avg_watch_duration_seconds")?,
        follower_increase_count: row.try_get("follower_increase_count")?,
        like_rate: row.try_get("like_rate")?,
        comment_rate: row.try_get("comment_rate")?,
        live_game_name: row.try_get("live_game_name")?,
        pulled_at: row.try_get("pulled_at")?,
        audit_result: row.try_get("audit_result")?,
        label: row.try_get("label")?,
        audit_extra: redact_audit_extra(row.try_get("audit_extra")?),
        last_seen_at: row.try_get("last_seen_at")?,
    })
}

fn page<T>(data: Vec<T>, total: i64, limit: i64, offset: i64) -> AuditExtraPageDto<T> {
    let returned = i64::try_from(data.len()).unwrap_or(i64::MAX);
    let next_offset = offset.saturating_add(returned);
    let has_more = next_offset < total;
    AuditExtraPageDto {
        data,
        meta: AuditExtraPageMetaDto {
            total,
            limit,
            offset,
            has_more,
            next_offset: has_more.then_some(next_offset),
        },
    }
}

/// 从任何对外 audit_extra 值中删除顶层机密字段 `key`。
///
/// 嵌套对象中的同名字段属于其对象自身的数据，不在这个顶层保密约定内。
pub(crate) fn redact_audit_extra(value: Value) -> Value {
    match value {
        Value::Object(mut object) => {
            object.remove(SENSITIVE_AUDIT_EXTRA_KEY);
            Value::Object(object)
        }
        other => other,
    }
}

fn redact_condition_map(conditions: BTreeMap<String, Value>) -> BTreeMap<String, Value> {
    conditions
        .into_iter()
        .filter(|(key, _)| key != SENSITIVE_AUDIT_EXTRA_KEY)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(conditions: BTreeMap<String, Value>) -> AuditExtraSearchRequest {
        AuditExtraSearchRequest {
            project_id: 1,
            activity_period_id: 2,
            conditions,
            audit_result: None,
            limit: None,
            offset: None,
        }
    }

    #[test]
    fn normalizes_pagination_and_audit_result_without_changing_json_types() {
        let conditions = BTreeMap::from([
            ("key".to_owned(), json!(true)),
            ("rok_key".to_owned(), json!(1)),
        ]);
        let mut input = request(conditions);
        input.audit_result = Some(" 审核通过 ".to_owned());
        input.limit = Some(9_999);
        input.offset = Some(-1);

        let normalized = input.normalized().unwrap();
        assert_eq!(normalized.audit_result.as_deref(), Some("审核通过"));
        assert_eq!(normalized.limit, Some(500));
        assert_eq!(normalized.offset, Some(0));
        assert_eq!(normalized.conditions["key"], json!(true));
        assert_eq!(normalized.conditions["rok_key"], json!(1));
    }

    #[test]
    fn rejects_empty_or_ambiguous_condition_keys() {
        assert!(request(BTreeMap::new()).normalized().is_err());
        assert!(
            request(BTreeMap::from([(" key ".to_owned(), json!(true))]))
                .normalized()
                .is_err()
        );
    }

    #[test]
    fn independent_pages_report_next_offset() {
        let first_page = page(vec![1, 2], 5, 2, 1);
        assert_eq!(first_page.meta.next_offset, Some(3));
        assert!(first_page.meta.has_more);

        let last = page(vec![1], 2, 2, 1);
        assert_eq!(last.meta.next_offset, None);
        assert!(!last.meta.has_more);
    }

    #[test]
    fn top_level_sensitive_key_is_removed_and_detected_for_cache_bypass() {
        let value = json!({
            "key": "top-secret",
            "visible": true,
            "nested": { "key": "nested-secret", "name": "safe" },
            "items": [{ "key": "array-secret", "value": 1 }]
        });
        assert_eq!(
            redact_audit_extra(value),
            json!({
                "visible": true,
                "nested": { "key": "nested-secret", "name": "safe" },
                "items": [{ "key": "array-secret", "value": 1 }]
            })
        );

        let sensitive = request(BTreeMap::from([("key".to_owned(), json!("top-secret"))]));
        assert!(sensitive.contains_sensitive_condition());
        let ordinary = request(BTreeMap::from([("rok_key".to_owned(), json!("ROK"))]));
        assert!(!ordinary.contains_sensitive_condition());
        let nested_only = request(BTreeMap::from([(
            "object".to_owned(),
            json!({ "key": "nested-business-value" }),
        )]));
        assert!(!nested_only.contains_sensitive_condition());
    }

    #[test]
    fn only_structured_conditions_are_used_for_gin_prefilter() {
        let input = request(BTreeMap::from([
            ("text".to_owned(), json!("value")),
            ("number".to_owned(), json!(1)),
            ("boolean".to_owned(), json!(true)),
            ("null".to_owned(), Value::Null),
            ("array".to_owned(), json!([1, 2])),
            ("object".to_owned(), json!({ "a": 1 })),
        ]));

        assert_eq!(
            input.indexable_conditions_json(),
            json!({
                "array": [1, 2],
                "object": { "a": 1 }
            })
        );
    }

    #[test]
    fn threshold_requests_allow_whole_period_and_validate_bounds() {
        let live = WeightedAcuLivePvRequest {
            project_id: 1,
            activity_period_id: 2,
            conditions: BTreeMap::new(),
            audit_result: Some(" 审核通过 ".to_owned()),
            weighted_average_acu_lt: 10.0,
        }
        .normalized()
        .unwrap();
        assert_eq!(live.audit_result.as_deref(), Some("审核通过"));
        assert!(!live.contains_sensitive_condition());

        let video = AuthorPlayVideoTotalRequest {
            project_id: 1,
            activity_period_id: 2,
            conditions: BTreeMap::from([("key".to_owned(), json!("secret"))]),
            audit_result: None,
            author_total_play_count_lt: 100_000,
        }
        .normalized()
        .unwrap();
        assert!(video.contains_sensitive_condition());

        assert!(
            WeightedAcuLivePvRequest {
                weighted_average_acu_lt: -0.1,
                ..live
            }
            .normalized()
            .is_err()
        );
        assert!(
            AuthorPlayVideoTotalRequest {
                author_total_play_count_lt: -1,
                ..video
            }
            .normalized()
            .is_err()
        );
    }
}
