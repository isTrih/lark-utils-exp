use crate::client::LarkClient;
use crate::lark::im::{FeishuImClient, ReceiveIdType, build_template_card};
use crate::lark::sheets::get_spreadsheet_token;
use crate::pipeline::bitable::read_optional_records_with_retry;
use crate::pipeline::sync::{find_field_value_by_name, value_to_plain_string};
use anyhow::{Context, anyhow};
use chrono::{NaiveDate, Utc};
use chrono_tz::Asia::Shanghai;
use open_lark::docs::base::bitable::Record as BitableRecord;
use salvo::oapi::ToSchema;
use serde::Serialize;
use serde_json::json;
use sqlx::{PgPool, Row};
use std::env;

const PROJECT_REPORT_TEMPLATE_ENV: &str = "PROJECT_REPORT_TEMPLATE_ID";
const PROJECT_REPORT_WITH_HOT_VIDEOS_TEMPLATE_ENV: &str =
    "PROJECT_REPORT_WITH_HOT_VIDEOS_TEMPLATE_ID";

/// 项目汇报卡片所需的数据库上下文。
#[derive(Debug, Clone)]
pub struct ProjectReportContext {
    pub activity_period_id: i64,
    pub project: String,
    pub period: String,
    pub bitable_url: String,
    pub cpm_table_id: Option<String>,
    pub video_play: i64,
    pub live_pv: i64,
}

/// CPM 配置表中的两个业务值。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectCpmValues {
    pub video_cpm: String,
    pub live_cpm: String,
}

/// 项目汇报卡片发送结果。
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct ProjectReportResult {
    pub activity_period_id: i64,
    pub project: String,
    pub period: String,
    pub template_id: String,
    pub video_play: String,
    pub video_cpm: String,
    pub live_pv: String,
    pub live_cpm: String,
    pub date: NaiveDate,
    pub message_id: Option<String>,
}

/// 读取当前启用项目的 CPM 配置和最新累计数据。
pub async fn load_project_report_context(
    pool: &PgPool,
    activity_period_id: i64,
) -> anyhow::Result<Option<ProjectReportContext>> {
    let row = sqlx::query(
        r#"
        WITH latest_video_metric AS (
            SELECT DISTINCT ON (m.content_config_id, m.video_id)
                m.content_config_id,
                m.video_id,
                m.play_count
            FROM video_daily_metric m
            JOIN xingtu_activity_content_config c
                ON c.content_config_id = m.content_config_id
            WHERE
                c.activity_period_id = $1
                AND c.content_type = 'video'
            ORDER BY
                m.content_config_id,
                m.video_id,
                m.stat_date DESC,
                m.imported_at DESC
        ),
        video_total AS (
            SELECT COALESCE(SUM(COALESCE(play_count, 0)), 0)::bigint AS video_play
            FROM latest_video_metric
        ),
        live_total AS (
            SELECT
                COALESCE(SUM(COALESCE(ls.live_exposure_pv, 0)), 0)::bigint AS live_pv
            FROM live_session ls
            JOIN xingtu_activity_content_config c
                ON c.content_config_id = ls.content_config_id
            WHERE
                c.activity_period_id = $1
                AND c.content_type = 'live'
        )
        SELECT
            p.activity_period_id,
            project.display_name AS project,
            p.period,
            p.bitable_url,
            p.cpm_table_id,
            video_total.video_play,
            live_total.live_pv
        FROM xingtu_activity_period p
        JOIN xingtu_project project ON project.project_id = p.project_id
        CROSS JOIN video_total
        CROSS JOIN live_total
        WHERE
            p.activity_period_id = $1
            AND p.is_active = true
        "#,
    )
    .bind(activity_period_id)
    .fetch_optional(pool)
    .await?;

    row.map(|row| {
        Ok(ProjectReportContext {
            activity_period_id: row.try_get("activity_period_id")?,
            project: row.try_get("project")?,
            period: row.try_get("period")?,
            bitable_url: row.try_get("bitable_url")?,
            cpm_table_id: row.try_get("cpm_table_id")?,
            video_play: row.try_get("video_play")?,
            live_pv: row.try_get("live_pv")?,
        })
    })
    .transpose()
}

/// 读取项目 CPM、组装模板变量并发送到指定群聊。
pub async fn send_project_report(
    lark: &LarkClient,
    context: ProjectReportContext,
    mission: &str,
    hot_videos: Option<&str>,
    chat_id: &str,
) -> anyhow::Result<ProjectReportResult> {
    let cpm_table_id = context
        .cpm_table_id
        .as_deref()
        .ok_or_else(|| anyhow!("项目 `{}` 未配置 cpm_table_id", context.period))?;
    let app_token =
        get_spreadsheet_token(&context.bitable_url).context("解析项目多维表 app_token 失败")?;
    let records = read_optional_records_with_retry(lark, &app_token, cpm_table_id)
        .await
        .context("读取项目 CPM 配置表失败")?;
    let cpm = extract_project_cpm_values(&records)?;
    let hot_videos = hot_videos.map(str::trim).filter(|value| !value.is_empty());
    let template_id = project_report_template_id(hot_videos)?;
    let date = Utc::now().with_timezone(&Shanghai).date_naive();
    let video_play = format_ten_thousands(context.video_play);
    let live_pv = format_ten_thousands(context.live_pv);
    let card = build_project_report_card(
        &template_id,
        &context,
        &cpm,
        ProjectReportCardValues {
            video_play: &video_play,
            live_pv: &live_pv,
            date,
            mission,
            hot_videos: hot_videos.unwrap_or_default(),
        },
    );
    let send_result = FeishuImClient::new(lark)
        .send_card(ReceiveIdType::ChatId, chat_id, card, None)
        .await
        .context("发送项目汇报卡片失败")?;

    Ok(ProjectReportResult {
        activity_period_id: context.activity_period_id,
        project: context.project,
        period: context.period,
        template_id,
        video_play,
        video_cpm: cpm.video_cpm,
        live_pv,
        live_cpm: cpm.live_cpm,
        date,
        message_id: send_result.message_id,
    })
}

fn project_report_template_id(hot_videos: Option<&str>) -> anyhow::Result<String> {
    let env_name = project_report_template_env_name(hot_videos);
    env::var(env_name)
        .map(|value| value.trim().to_string())
        .map_err(|_| anyhow!("未配置环境变量 {env_name}"))
        .and_then(|value| {
            if value.is_empty() {
                Err(anyhow!("环境变量 {env_name} 不能为空"))
            } else {
                Ok(value)
            }
        })
}

fn project_report_template_env_name(hot_videos: Option<&str>) -> &'static str {
    if hot_videos.is_some_and(|value| !value.trim().is_empty()) {
        PROJECT_REPORT_WITH_HOT_VIDEOS_TEMPLATE_ENV
    } else {
        PROJECT_REPORT_TEMPLATE_ENV
    }
}

fn extract_project_cpm_values(records: &[BitableRecord]) -> anyhow::Result<ProjectCpmValues> {
    let mut video_cpm = None;
    let mut live_cpm = None;

    for record in records {
        let Some(fields) = record.fields.as_object() else {
            continue;
        };

        if video_cpm.is_none() {
            video_cpm = find_field_value_by_name(fields, "视频CPM")
                .and_then(value_to_plain_string)
                .filter(|value| !value.trim().is_empty());
        }
        if live_cpm.is_none() {
            live_cpm = find_field_value_by_name(fields, "直播CPM")
                .and_then(value_to_plain_string)
                .filter(|value| !value.trim().is_empty());
        }
        if video_cpm.is_some() && live_cpm.is_some() {
            break;
        }
    }

    Ok(ProjectCpmValues {
        video_cpm: format_cpm(
            "视频CPM",
            &video_cpm.ok_or_else(|| anyhow!("CPM 配置表中找不到有效的 `视频CPM`"))?,
        )?,
        live_cpm: format_cpm(
            "直播CPM",
            &live_cpm.ok_or_else(|| anyhow!("CPM 配置表中找不到有效的 `直播CPM`"))?,
        )?,
    })
}

fn format_ten_thousands(value: i64) -> String {
    format!("{:.2}", value as f64 / 10_000.0)
}

fn format_cpm(field_name: &str, value: &str) -> anyhow::Result<String> {
    let value = value
        .trim()
        .parse::<f64>()
        .with_context(|| format!("CPM 配置表字段 `{field_name}` 不是有效数字"))?;
    Ok(format!("{value:.2}"))
}

struct ProjectReportCardValues<'a> {
    video_play: &'a str,
    live_pv: &'a str,
    date: NaiveDate,
    mission: &'a str,
    hot_videos: &'a str,
}

fn build_project_report_card(
    template_id: &str,
    context: &ProjectReportContext,
    cpm: &ProjectCpmValues,
    values: ProjectReportCardValues<'_>,
) -> serde_json::Value {
    build_template_card(
        template_id,
        json!({
            "video_play": values.video_play,
            "video_cpm": cpm.video_cpm,
            "live_pv": values.live_pv,
            "live_cpm": cpm.live_cpm,
            "date": values.date.to_string(),
            "mission": values.mission,
            "hot_videos": values.hot_videos,
            "period": context.period,
            "project_name": context.project
        }),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn extracts_video_and_live_cpm_from_bitable_records() {
        let records = vec![record(json!({
            "视频CPM": 12.5,
            "直播CPM": "8.80"
        }))];

        assert_eq!(
            extract_project_cpm_values(&records).unwrap(),
            ProjectCpmValues {
                video_cpm: "12.50".to_string(),
                live_cpm: "8.80".to_string(),
            }
        );
    }

    #[test]
    fn project_report_card_uses_expected_template_variables() {
        const TEST_TEMPLATE_ID: &str = "example-hot-video-template-id";
        let context = ProjectReportContext {
            activity_period_id: 1,
            project: "ROK".to_string(),
            period: "2026年7月第十四期".to_string(),
            bitable_url: "https://example.feishu.cn/wiki/token".to_string(),
            cpm_table_id: Some("tbl_cpm".to_string()),
            video_play: 1234,
            live_pv: 5678,
        };
        let cpm = ProjectCpmValues {
            video_cpm: "12.50".to_string(),
            live_cpm: "8.80".to_string(),
        };
        let card = build_project_report_card(
            TEST_TEMPLATE_ID,
            &context,
            &cpm,
            ProjectReportCardValues {
                video_play: &format_ten_thousands(context.video_play),
                live_pv: &format_ten_thousands(context.live_pv),
                date: NaiveDate::from_ymd_opt(2026, 7, 27).unwrap(),
                mission: "**今日事项**",
                hot_videos: "- 热点视频",
            },
        );
        let variables = &card["data"]["template_variable"];

        assert_eq!(card["data"]["template_id"], TEST_TEMPLATE_ID);
        assert_eq!(variables["video_play"], "0.12");
        assert_eq!(variables["video_cpm"], "12.50");
        assert_eq!(variables["live_pv"], "0.57");
        assert_eq!(variables["live_cpm"], "8.80");
        assert_eq!(variables["date"], "2026-07-27");
        assert_eq!(variables["mission"], "**今日事项**");
        assert_eq!(variables["hot_videos"], "- 热点视频");
        assert_eq!(variables["period"], "2026年7月第十四期");
        assert_eq!(variables["project_name"], "ROK");
    }

    #[test]
    fn project_report_template_environment_follows_hot_video_presence() {
        assert_eq!(
            project_report_template_env_name(None),
            PROJECT_REPORT_TEMPLATE_ENV
        );
        assert_eq!(
            project_report_template_env_name(Some("  ")),
            PROJECT_REPORT_TEMPLATE_ENV
        );
        assert_eq!(
            project_report_template_env_name(Some("- 热点")),
            PROJECT_REPORT_WITH_HOT_VIDEOS_TEMPLATE_ENV
        );
    }

    #[test]
    fn project_report_numbers_use_expected_display_precision() {
        assert_eq!(format_ten_thousands(17_906_033), "1790.60");
        assert_eq!(format_ten_thousands(4_023_365), "402.34");
        assert_eq!(format_cpm("视频CPM", "4.199419812948205").unwrap(), "4.20");
        assert_eq!(format_cpm("直播CPM", "22.30882336561178").unwrap(), "22.31");
    }

    fn record(fields: serde_json::Value) -> BitableRecord {
        BitableRecord {
            record_id: "rec_cpm".to_string(),
            fields,
            created_by: None,
            created_time: None,
            last_modified_by: None,
            last_modified_time: None,
            shared_url: None,
            record_url: None,
        }
    }
}
