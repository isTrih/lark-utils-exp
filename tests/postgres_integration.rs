use chrono::NaiveDate;
use lark_exp::server::audit_extra_query::{AuditExtraSearchRequest, search as search_audit_extra};
use lark_exp::server::cache::QueryCache;
use lark_exp::server::query::{
    OperationalContentQuery, VideoAnalyticsQuery, VideoGrowthQuery, VideoLabelSummaryQuery,
    list_video_contents_v2, list_video_trace_metrics, list_videos_with_metrics, top_video_growth,
    video_label_summary, video_summary,
};
use lark_exp::workflow_run::WorkflowRunRepository;
use lark_exp::xingtu::data_import::{AuditResultUpdate, XingtuDataImportRepository};
use serde_json::json;
use sqlx::migrate::Migrator;
use sqlx::postgres::PgPoolOptions;
use sqlx::types::Json;
use std::borrow::Cow;
use std::collections::BTreeMap;
use std::time::Duration;

fn audit_extra_request(
    project_id: i64,
    activity_period_id: i64,
    conditions: serde_json::Value,
    audit_result: Option<&str>,
) -> AuditExtraSearchRequest {
    AuditExtraSearchRequest {
        project_id,
        activity_period_id,
        conditions: serde_json::from_value::<BTreeMap<String, serde_json::Value>>(conditions)
            .expect("conditions fixture must be an object"),
        audit_result: audit_result.map(ToOwned::to_owned),
        limit: Some(100),
        offset: Some(0),
    }
}

/// CI 通过 TEST_DATABASE_URL 注入一次性 PostgreSQL；本地未配置时安全跳过。
#[tokio::test]
async fn operational_reliability_database_contracts() -> anyhow::Result<()> {
    let Ok(database_url) = std::env::var("TEST_DATABASE_URL") else {
        eprintln!("TEST_DATABASE_URL 未配置，跳过 PostgreSQL 集成测试");
        return Ok(());
    };
    let pool = PgPoolOptions::new()
        .max_connections(5)
        .connect(&database_url)
        .await?;
    let migrations = sqlx::migrate!("./migrations");
    let migrations_before_audit_extra_normalization = Migrator {
        migrations: Cow::Owned(
            migrations
                .iter()
                .filter(|migration| migration.version < 20_260_819_000_300)
                .cloned()
                .collect(),
        ),
        ..Migrator::DEFAULT
    };
    migrations_before_audit_extra_normalization
        .run(&pool)
        .await?;

    let repository = WorkflowRunRepository::new(pool.clone());
    let lock = repository
        .try_acquire_global_lock()
        .await?
        .expect("首个执行器应拿到 advisory lock");
    assert!(repository.try_acquire_global_lock().await?.is_none());
    lock.rollback().await?;
    repository
        .try_acquire_global_lock()
        .await?
        .expect("事务结束后 advisory lock 必须释放")
        .rollback()
        .await?;

    let request_id = format!("postgres-integration-{}", std::process::id());
    let run_id = repository
        .start_run("integration", None, "test", Some(&request_id))
        .await?;
    let step_id = repository
        .start_step(run_id, None, "database_contract", None)
        .await?;
    repository
        .finish_step_success(step_id, json!({ "rows": 1 }))
        .await?;
    repository
        .finish_run_success(run_id, json!({ "ok": true }))
        .await?;
    let run = repository
        .list_runs(100, 0)
        .await?
        .into_iter()
        .find(|run| run.workflow_run_id == run_id)
        .expect("运行台账应可查询");
    assert_eq!(run.status, "succeeded");
    assert_eq!(repository.list_steps(run_id).await?.len(), 1);

    let scope = format!("integration-{run_id}");
    assert!(
        repository
            .claim_notification("test", &scope, "payload-a", Duration::from_secs(60))
            .await?
    );
    repository.mark_notification_sent("test", &scope).await?;
    assert!(
        !repository
            .claim_notification("test", &scope, "payload-a", Duration::from_secs(60))
            .await?
    );
    assert!(
        repository
            .claim_notification("test", &scope, "payload-b", Duration::from_secs(60))
            .await?
    );

    let cache = QueryCache::new(pool.clone());
    let before: i64 =
        sqlx::query_scalar("SELECT revision FROM query_cache_revision WHERE singleton = true")
            .fetch_one(&pool)
            .await?;
    cache.invalidate_all_shared().await?;
    let after: i64 =
        sqlx::query_scalar("SELECT revision FROM query_cache_revision WHERE singleton = true")
            .fetch_one(&pool)
            .await?;
    assert_eq!(after, before + 1);

    let nonce = format!("integration-nonce-{run_id}");
    let sensitive_audit_key = "integration-confidential-value";
    sqlx::query(
        "INSERT INTO data_sync_request_nonce (nonce, request_timestamp, expires_at) VALUES ($1, now(), now() + interval '10 minutes')",
    )
    .bind(&nonce)
    .execute(&pool)
    .await?;
    assert!(
        sqlx::query(
            "INSERT INTO data_sync_request_nonce (nonce, request_timestamp, expires_at) VALUES ($1, now(), now() + interval '10 minutes')",
        )
        .bind(&nonce)
        .execute(&pool)
        .await
        .is_err()
    );

    let fixture = format!("label-report-{run_id}");
    let project_id: i64 = sqlx::query_scalar(
        "INSERT INTO xingtu_project (project_key, display_name, notification_receive_id) VALUES ($1, $1, 'integration-chat') RETURNING project_id",
    )
    .bind(&fixture)
    .fetch_one(&pool)
    .await?;
    let other_project_id: i64 = sqlx::query_scalar(
        "INSERT INTO xingtu_project (project_key, display_name, notification_receive_id) VALUES ($1, $1, 'integration-other-chat') RETURNING project_id",
    )
    .bind(format!("{fixture}-other"))
    .fetch_one(&pool)
    .await?;
    let project_account_id = format!("project-account-{run_id}");
    sqlx::query(
        r#"
        INSERT INTO xingtu_project_account (
            xingtu_account_id, project_id, is_default
        ) VALUES ($1, $2, true)
        "#,
    )
    .bind(&project_account_id)
    .bind(project_id)
    .execute(&pool)
    .await?;
    sqlx::query(
        r#"
        INSERT INTO xingtu_project_auditor (
            project_id, auditor_name, auditor_id
        ) VALUES ($1, 'Integration Auditor', $2)
        "#,
    )
    .bind(project_id)
    .bind(format!("auditor-{run_id}"))
    .execute(&pool)
    .await?;
    let activity_period_id: i64 = sqlx::query_scalar(
        r#"
        INSERT INTO xingtu_activity_period (
            project_id, xingtu_account_id, period, period_code, task_month, bitable_url
        ) VALUES ($1, $2, $3, $4, DATE '2026-08-01', 'https://example.test/base')
        RETURNING activity_period_id
        "#,
    )
    .bind(project_id)
    .bind(&project_account_id)
    .bind(&fixture)
    .bind(&fixture)
    .fetch_one(&pool)
    .await?;
    let child_counts: (i64, i64, i64) = sqlx::query_as(
        r#"
        SELECT
            (SELECT COUNT(*) FROM xingtu_project_account WHERE project_id = $1),
            (SELECT COUNT(*) FROM xingtu_project_auditor WHERE project_id = $1),
            (SELECT COUNT(*) FROM xingtu_activity_period WHERE project_id = $1)
        "#,
    )
    .bind(project_id)
    .fetch_one(&pool)
    .await?;
    assert_eq!(child_counts, (1, 1, 1));
    let content_config_id: i64 = sqlx::query_scalar(
        r#"
        INSERT INTO xingtu_activity_content_config (
            activity_period_id, content_type, xingtu_task_id, main_table_id, audit_table_id
        ) VALUES ($1, 'video', $2, 'integration-main', 'integration-audit')
        RETURNING content_config_id
        "#,
    )
    .bind(activity_period_id)
    .bind(&fixture)
    .fetch_one(&pool)
    .await?;
    let live_content_config_id: i64 = sqlx::query_scalar(
        r#"
        INSERT INTO xingtu_activity_content_config (
            activity_period_id, content_type, xingtu_task_id, main_table_id, audit_table_id
        ) VALUES ($1, 'live', $2, 'integration-live-main', 'integration-live-audit')
        RETURNING content_config_id
        "#,
    )
    .bind(activity_period_id)
    .bind(format!("{fixture}-live"))
    .fetch_one(&pool)
    .await?;
    let baseline_source_id: i64 = sqlx::query_scalar(
        r#"
        INSERT INTO xingtu_feishu_source (
            content_config_id, content_type, feishu_sheet_url, trigger_type, stat_date
        ) VALUES ($1, 'video', $2, 'manual', DATE '2026-08-02')
        RETURNING feishu_source_id
        "#,
    )
    .bind(content_config_id)
    .bind(format!("https://example.test/{fixture}/baseline"))
    .fetch_one(&pool)
    .await?;
    let report_source_id: i64 = sqlx::query_scalar(
        r#"
        INSERT INTO xingtu_feishu_source (
            content_config_id, content_type, feishu_sheet_url, trigger_type, stat_date
        ) VALUES ($1, 'video', $2, 'manual', DATE '2026-08-09')
        RETURNING feishu_source_id
        "#,
    )
    .bind(content_config_id)
    .bind(format!("https://example.test/{fixture}/report"))
    .fetch_one(&pool)
    .await?;
    let live_source_id: i64 = sqlx::query_scalar(
        r#"
        INSERT INTO xingtu_feishu_source (
            content_config_id, content_type, feishu_sheet_url, trigger_type, stat_date
        ) VALUES ($1, 'live', $2, 'manual', DATE '2026-08-10')
        RETURNING feishu_source_id
        "#,
    )
    .bind(live_content_config_id)
    .bind(format!("https://example.test/{fixture}/live"))
    .fetch_one(&pool)
    .await?;
    sqlx::query(
        r#"
        INSERT INTO video_content (
            content_config_id, video_id, publish_time, author_name, author_uid,
            title, audit_result, label
        ) VALUES
            ($1, 'video-a', TIMESTAMP '2026-08-03 00:00:00', '作者甲', 'author-a',
                '攻略开场', '审核通过', '攻略'),
            ($1, 'video-b', TIMESTAMP '2026-08-09 23:59:59.999999', '作者乙', 'author-b',
                '攻略收尾', '审核通过', '攻略'),
            ($1, 'video-c', TIMESTAMP '2026-08-10 00:00:00', '作者丙', 'author-c',
                '未标注内容', '审核通过', NULL)
        "#,
    )
    .bind(content_config_id)
    .execute(&pool)
    .await?;
    sqlx::query(
        r#"
        INSERT INTO video_content (
            content_config_id, video_id, publish_time, author_name, author_uid,
            title, audit_result, label, audit_extra
        ) VALUES (
            $1, 'video-e', TIMESTAMP '2026-08-10 13:00:00', '作者戊', 'author-e',
            '尚无指标测试', '审核通过', '其他', $2
        )
        "#,
    )
    .bind(content_config_id)
    .bind(Json(json!({
        "rok_key": "ROK",
        "key": {
            "type": 1,
            "value": [{ "text": sensitive_audit_key, "type": "text" }]
        },
        "number": { "type": 2, "value": 1 },
        "enabled": { "type": 7, "value": true }
    })))
    .execute(&pool)
    .await?;

    let audit_repository = XingtuDataImportRepository::new(pool.clone());
    let audit_extra = json!({
        "塔塔二创": 1,
        "是否推荐": true,
        "备注": "integration",
        "rok_key": "ROK",
        "key": sensitive_audit_key,
        "number": 1,
        "enabled": true,
        "nullable": null,
        "array": [1, 2],
        "object": { "a": 1 }
    });
    assert_eq!(
        audit_repository
            .update_audit_results(
                content_config_id,
                "video",
                &[AuditResultUpdate {
                    unique_key: "video-a".to_owned(),
                    audit_result: "审核通过".to_owned(),
                    label: Some("攻略".to_owned()),
                    audit_extra: audit_extra.clone(),
                }],
            )
            .await?,
        1
    );
    sqlx::query(
        r#"
        INSERT INTO video_content (
            content_config_id, video_id, publish_time, author_name, author_uid,
            title, audit_result, label, audit_extra
        ) VALUES (
            $1, 'video-d', TIMESTAMP '2026-08-10 12:00:00', '作者丁', 'author-d',
            '严格匹配测试', '不通过', '其他', $2
        )
        "#,
    )
    .bind(content_config_id)
    .bind(Json(json!({
        "rok_key": "ROK",
        "key": sensitive_audit_key,
        "number": 1,
        "enabled": true,
        "array": [1, 2, 3],
        "object": { "a": 1, "b": 2 }
    })))
    .execute(&pool)
    .await?;
    sqlx::query(
        r#"
        INSERT INTO live_session (
            content_config_id, feishu_source_id, live_room_id, start_time,
            anchor_name, anchor_uid, title, cumulative_viewer_count,
            live_exposure_pv, acu, audit_result, audit_extra
        ) VALUES
            ($1, $2, 'live-pass', TIMESTAMP '2026-08-10 10:00:00',
                '主播甲', 'anchor-a', '通过直播', 9999, 700, 100, '审核通过',
                $3),
            ($1, $2, 'live-fail', TIMESTAMP '2026-08-10 11:00:00',
                '主播乙', 'anchor-b', '不通过直播', 8888, 300, 80, '不通过',
                $4)
        "#,
    )
    .bind(live_content_config_id)
    .bind(live_source_id)
    .bind(Json(json!({
        "rok_key": "ROK",
        "key": sensitive_audit_key,
        "number": 1,
        "enabled": true
    })))
    .bind(Json(json!({
        "rok_key": "ROK",
        "key": {
            "type": 1,
            "value": [
                { "text": "integration-confidential-", "type": "text" },
                { "text": "value", "type": "text" }
            ]
        },
        "number": { "type": 2, "value": 1 },
        "enabled": { "type": 7, "value": true }
    })))
    .execute(&pool)
    .await?;
    sqlx::query(
        r#"
        INSERT INTO video_daily_metric (
            content_config_id, feishu_source_id, stat_date, video_id,
            play_count, valid_play_count, like_count, comment_count, share_count
        ) VALUES
            ($1, $2, DATE '2026-08-02', 'video-a', 100, 50, 10, 2, 1),
            ($1, $3, DATE '2026-08-03', 'video-a', 150, 75, 15, 3, 2),
            ($1, $3, DATE '2026-08-09', 'video-b', 200, 100, 20, 4, 3),
            ($1, $3, DATE '2026-08-04', 'video-c', 30, 15, 3, 1, 1),
            ($1, $3, DATE '2026-08-10', 'video-d', 400, 200, 40, 8, 4)
        "#,
    )
    .bind(content_config_id)
    .bind(baseline_source_id)
    .bind(report_source_id)
    .execute(&pool)
    .await?;

    let legacy_video_extra: Json<serde_json::Value> = sqlx::query_scalar(
        "SELECT audit_extra FROM video_content WHERE content_config_id = $1 AND video_id = 'video-e'",
    )
    .bind(content_config_id)
    .fetch_one(&pool)
    .await?;
    assert!(legacy_video_extra.0["key"].is_object());

    migrations.run(&pool).await?;

    let normalized_video_extra: Json<serde_json::Value> = sqlx::query_scalar(
        "SELECT audit_extra FROM video_content WHERE content_config_id = $1 AND video_id = 'video-e'",
    )
    .bind(content_config_id)
    .fetch_one(&pool)
    .await?;
    assert_eq!(normalized_video_extra.0["key"], json!(sensitive_audit_key));
    assert_eq!(normalized_video_extra.0["number"], json!(1));
    assert_eq!(normalized_video_extra.0["enabled"], json!(true));

    let normalized_live_extra: Json<serde_json::Value> = sqlx::query_scalar(
        "SELECT audit_extra FROM live_session WHERE content_config_id = $1 AND live_room_id = 'live-fail'",
    )
    .bind(live_content_config_id)
    .fetch_one(&pool)
    .await?;
    assert_eq!(normalized_live_extra.0["key"], json!(sensitive_audit_key));
    assert_eq!(normalized_live_extra.0["number"], json!(1));
    assert_eq!(normalized_live_extra.0["enabled"], json!(true));

    // 模拟滚动发布期间旧实例再次写入富文本包装；新查询仍按业务字符串命中。
    sqlx::query(
        "UPDATE video_content SET audit_extra = jsonb_set(audit_extra, '{key}', $2) WHERE content_config_id = $1 AND video_id = 'video-e'",
    )
    .bind(content_config_id)
    .bind(Json(json!({
        "type": 1,
        "value": [{ "text": sensitive_audit_key, "type": "text" }]
    })))
    .execute(&pool)
    .await?;

    let all_audit_extra = search_audit_extra(
        &pool,
        audit_extra_request(
            project_id,
            activity_period_id,
            json!({ "rok_key": "ROK", "key": sensitive_audit_key }),
            None,
        ),
    )
    .await?
    .expect("project-period scope should exist");
    assert_eq!(all_audit_extra.summary.video_count, 3);
    assert_eq!(all_audit_extra.summary.live_session_count, 2);
    assert_eq!(all_audit_extra.summary.total_content_count, 5);
    assert_eq!(all_audit_extra.summary.video_with_metric_count, 2);
    assert_eq!(all_audit_extra.summary.total_play_count, 550);
    assert_eq!(all_audit_extra.summary.total_live_exposure_pv, 1_000);
    assert_eq!(all_audit_extra.videos.meta.total, 3);
    assert!(!serde_json::to_string(&all_audit_extra)?.contains(sensitive_audit_key));
    assert!(!all_audit_extra.filters.conditions.contains_key("key"));
    assert!(
        all_audit_extra
            .videos
            .data
            .iter()
            .all(|video| video.audit_extra.get("key").is_none())
    );
    assert!(
        all_audit_extra
            .live_sessions
            .data
            .iter()
            .all(|live| live.audit_extra.get("key").is_none())
    );
    assert_eq!(
        all_audit_extra
            .videos
            .data
            .iter()
            .find(|video| video.video_id == "video-a")
            .and_then(|video| video.latest_metric.as_ref())
            .and_then(|metric| metric.play_count),
        Some(150)
    );
    assert_eq!(
        all_audit_extra
            .live_sessions
            .data
            .iter()
            .map(|live| live.live_exposure_pv.unwrap_or_default())
            .sum::<i64>(),
        1_000
    );

    for single_condition in [
        json!({ "rok_key": "ROK" }),
        json!({ "key": sensitive_audit_key }),
    ] {
        let result = search_audit_extra(
            &pool,
            audit_extra_request(project_id, activity_period_id, single_condition, None),
        )
        .await?
        .unwrap();
        assert_eq!(result.summary.video_count, 3);
        assert_eq!(result.summary.live_session_count, 2);
    }

    let numeric_condition = search_audit_extra(
        &pool,
        audit_extra_request(project_id, activity_period_id, json!({ "number": 1 }), None),
    )
    .await?
    .unwrap();
    assert_eq!(numeric_condition.summary.total_content_count, 5);

    let boolean_condition = search_audit_extra(
        &pool,
        audit_extra_request(
            project_id,
            activity_period_id,
            json!({ "enabled": true }),
            None,
        ),
    )
    .await?
    .unwrap();
    assert_eq!(boolean_condition.summary.total_content_count, 5);

    let wrong_case = search_audit_extra(
        &pool,
        audit_extra_request(
            project_id,
            activity_period_id,
            json!({ "rok_key": "rok" }),
            None,
        ),
    )
    .await?
    .unwrap();
    assert_eq!(wrong_case.summary.total_content_count, 0);

    let mut paged_request = audit_extra_request(
        project_id,
        activity_period_id,
        json!({ "rok_key": "ROK", "key": sensitive_audit_key }),
        None,
    );
    paged_request.limit = Some(1);
    paged_request.offset = Some(1);
    let paged_audit_extra = search_audit_extra(&pool, paged_request).await?.unwrap();
    assert_eq!(paged_audit_extra.summary.total_content_count, 5);
    assert_eq!(paged_audit_extra.videos.data.len(), 1);
    assert_eq!(paged_audit_extra.videos.meta.total, 3);
    assert_eq!(paged_audit_extra.videos.meta.offset, 1);
    assert!(paged_audit_extra.videos.meta.has_more);
    assert_eq!(paged_audit_extra.videos.meta.next_offset, Some(2));
    assert_eq!(paged_audit_extra.live_sessions.data.len(), 1);

    let approved_audit_extra = search_audit_extra(
        &pool,
        audit_extra_request(
            project_id,
            activity_period_id,
            json!({ "rok_key": "ROK", "key": sensitive_audit_key }),
            Some("审核通过"),
        ),
    )
    .await?
    .unwrap();
    assert_eq!(approved_audit_extra.summary.video_count, 2);
    assert_eq!(approved_audit_extra.summary.live_session_count, 1);
    assert_eq!(approved_audit_extra.summary.video_with_metric_count, 1);
    assert_eq!(approved_audit_extra.summary.total_play_count, 150);
    assert_eq!(approved_audit_extra.summary.total_live_exposure_pv, 700);

    let rejected_audit_extra = search_audit_extra(
        &pool,
        audit_extra_request(
            project_id,
            activity_period_id,
            json!({ "rok_key": "ROK", "key": sensitive_audit_key }),
            Some("不通过"),
        ),
    )
    .await?
    .unwrap();
    assert_eq!(rejected_audit_extra.summary.total_play_count, 400);
    assert_eq!(rejected_audit_extra.summary.total_live_exposure_pv, 300);

    for exact_nested_condition in [json!({ "array": [1, 2] }), json!({ "object": { "a": 1 } })] {
        let exact_nested = search_audit_extra(
            &pool,
            audit_extra_request(project_id, activity_period_id, exact_nested_condition, None),
        )
        .await?
        .unwrap();
        assert_eq!(exact_nested.summary.video_count, 1);
        assert_eq!(exact_nested.videos.data[0].video_id, "video-a");
    }

    let reversed_array = search_audit_extra(
        &pool,
        audit_extra_request(
            project_id,
            activity_period_id,
            json!({ "array": [2, 1] }),
            None,
        ),
    )
    .await?
    .unwrap();
    assert_eq!(reversed_array.summary.video_count, 0);

    let json_null = search_audit_extra(
        &pool,
        audit_extra_request(
            project_id,
            activity_period_id,
            json!({ "nullable": null }),
            None,
        ),
    )
    .await?
    .unwrap();
    assert_eq!(json_null.summary.video_count, 1);
    assert_eq!(json_null.videos.data[0].video_id, "video-a");

    for non_matching_conditions in [json!({ "number": "1" }), json!({ "key": true })] {
        let result = search_audit_extra(
            &pool,
            audit_extra_request(
                project_id,
                activity_period_id,
                non_matching_conditions,
                None,
            ),
        )
        .await?
        .unwrap();
        assert_eq!(result.summary.total_content_count, 0);
    }
    assert!(
        search_audit_extra(
            &pool,
            audit_extra_request(
                other_project_id,
                activity_period_id,
                json!({ "key": sensitive_audit_key }),
                None,
            ),
        )
        .await?
        .is_none()
    );

    let date_from = NaiveDate::from_ymd_opt(2026, 8, 3).unwrap();
    let date_to = NaiveDate::from_ymd_opt(2026, 8, 9).unwrap();
    let videos = list_video_contents_v2(
        &pool,
        OperationalContentQuery {
            activity_period_id: Some(activity_period_id),
            label: Some("攻略".to_owned()),
            date_from: Some(date_from),
            date_to: Some(date_to),
            limit: Some(50),
            ..OperationalContentQuery::default()
        },
    )
    .await?;
    assert_eq!(videos.meta.total, 2);
    assert!(
        videos
            .data
            .iter()
            .all(|video| video.label.as_deref() == Some("攻略"))
    );
    assert!(videos.data.iter().any(|video| video.video_id == "video-b"));
    let mut expected_public_audit_extra = audit_extra.clone();
    expected_public_audit_extra
        .as_object_mut()
        .unwrap()
        .remove("key");
    assert_eq!(
        videos
            .data
            .iter()
            .find(|video| video.video_id == "video-a")
            .unwrap()
            .audit_extra,
        expected_public_audit_extra
    );

    let analytics_query = VideoAnalyticsQuery {
        content_config_id: Some(content_config_id),
        video_id: None,
        author_uid: None,
        author_name: None,
        label: Some("攻略".to_owned()),
        date: None,
        date_from: Some(date_from),
        date_to: Some(date_to),
        limit: Some(50),
        offset: Some(0),
    };
    let analytics_summary = video_summary(&pool, analytics_query.clone()).await?;
    assert_eq!(analytics_summary.play_count, 250);
    assert_eq!(analytics_summary.content_count, 2);
    assert_eq!(
        list_videos_with_metrics(&pool, analytics_query.clone())
            .await?
            .len(),
        2
    );
    assert!(
        list_video_trace_metrics(&pool, analytics_query)
            .await?
            .is_empty()
    );
    let growth = top_video_growth(
        &pool,
        VideoGrowthQuery {
            content_config_id: Some(content_config_id),
            author_uid: None,
            author_name: None,
            label: Some("攻略".to_owned()),
            date: None,
            date_from: Some(date_from),
            date_to: Some(date_to),
            limit: Some(10),
        },
    )
    .await?;
    assert_eq!(growth.len(), 2);
    assert_eq!(growth[0].video_id, "video-b");

    let summary = video_label_summary(
        &pool,
        VideoLabelSummaryQuery {
            activity_period_id: Some(activity_period_id),
            content_config_id: Some(content_config_id),
            status: Some("审核通过".to_owned()),
            label: None,
            date_from,
            date_to,
        },
    )
    .await?;
    assert_eq!(summary.range.start_at.to_string(), "2026-08-03 00:00:00");
    assert_eq!(summary.range.end_at.to_string(), "2026-08-09 23:59:59");
    let strategy = summary
        .data
        .iter()
        .find(|item| item.label.as_deref() == Some("攻略"))
        .expect("应返回攻略标签聚合");
    assert_eq!(strategy.published_video_count, 2);
    assert_eq!(strategy.active_video_count, 2);
    assert_eq!(strategy.author_count, 2);
    assert_eq!(strategy.play_growth, 250);
    assert_eq!(strategy.valid_play_growth, 125);
    assert_eq!(strategy.like_growth, 25);
    assert_eq!(strategy.comment_growth, 5);
    assert_eq!(strategy.share_growth, 4);
    let unlabeled = summary
        .data
        .iter()
        .find(|item| item.label.is_none())
        .expect("应单独返回未标注聚合");
    assert_eq!(unlabeled.label_name, "未标注");
    assert_eq!(unlabeled.published_video_count, 0);
    assert_eq!(unlabeled.active_video_count, 1);
    assert_eq!(summary.total.published_video_count, 2);
    assert_eq!(summary.total.active_video_count, 3);
    assert_eq!(summary.total.play_growth, 280);
    assert_eq!(summary.total.valid_play_growth, 140);
    assert_eq!(summary.total.like_growth, 28);
    assert_eq!(summary.total.comment_growth, 6);
    assert_eq!(summary.total.share_growth, 5);

    sqlx::query("DELETE FROM video_daily_metric WHERE content_config_id = $1")
        .bind(content_config_id)
        .execute(&pool)
        .await?;
    sqlx::query("DELETE FROM live_session WHERE content_config_id = $1")
        .bind(live_content_config_id)
        .execute(&pool)
        .await?;
    sqlx::query("DELETE FROM video_content WHERE content_config_id = $1")
        .bind(content_config_id)
        .execute(&pool)
        .await?;
    sqlx::query("DELETE FROM xingtu_feishu_source WHERE content_config_id IN ($1, $2)")
        .bind(content_config_id)
        .bind(live_content_config_id)
        .execute(&pool)
        .await?;
    sqlx::query("DELETE FROM xingtu_activity_period WHERE activity_period_id = $1")
        .bind(activity_period_id)
        .execute(&pool)
        .await?;
    sqlx::query("DELETE FROM xingtu_project WHERE project_id = $1")
        .bind(other_project_id)
        .execute(&pool)
        .await?;

    sqlx::query("DELETE FROM data_sync_request_nonce WHERE nonce = $1")
        .bind(&nonce)
        .execute(&pool)
        .await?;
    sqlx::query(
        "DELETE FROM notification_delivery_guard WHERE category = 'test' AND scope_key = $1",
    )
    .bind(&scope)
    .execute(&pool)
    .await?;
    sqlx::query("DELETE FROM workflow_run WHERE workflow_run_id = $1")
        .bind(run_id)
        .execute(&pool)
        .await?;
    Ok(())
}
