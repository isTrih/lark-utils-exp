use crate::server::state::AppState;
use crate::workflow::WorkflowKind;
use chrono::{DateTime, Duration as ChronoDuration, NaiveTime, TimeZone, Utc};
use chrono_tz::Asia::Shanghai;
use std::{sync::Arc, time::Duration};
use tokio::time::{Instant, sleep_until};

/// 启动内置调度器。
///
/// 生产环境也可以改成由企业后端调度器调用 HTTP 手动执行接口；
/// 这里先落地服务自带调度，方便独立运行和调试。
pub fn spawn_scheduler(state: Arc<AppState>) {
    tracing::info!(
        "内置调度器已启动：periodic 下一次执行 [{}, {}, {}]，morning 下一次执行 {}，night 下一次执行 {}，登录态巡检下一次执行 {}",
        next_daily_local_time(NaiveTime::from_hms_opt(3, 0, 0).expect("valid time")),
        next_daily_local_time(NaiveTime::from_hms_opt(15, 0, 0).expect("valid time")),
        next_daily_local_time(NaiveTime::from_hms_opt(21, 0, 0).expect("valid time")),
        next_daily_local_time(NaiveTime::from_hms_opt(9, 0, 0).expect("valid time")),
        next_daily_local_time(NaiveTime::from_hms_opt(23, 59, 0).expect("valid time")),
        next_daily_local_time(NaiveTime::from_hms_opt(0, 30, 0).expect("valid time")),
    );
    for time in [
        NaiveTime::from_hms_opt(3, 0, 0).expect("valid time"),
        NaiveTime::from_hms_opt(15, 0, 0).expect("valid time"),
        NaiveTime::from_hms_opt(21, 0, 0).expect("valid time"),
    ] {
        tokio::spawn(run_daily_workflow_loop(
            state.clone(),
            WorkflowKind::Periodic,
            time,
        ));
    }
    tokio::spawn(run_daily_workflow_loop(
        state.clone(),
        WorkflowKind::Morning,
        NaiveTime::from_hms_opt(9, 0, 0).expect("valid time"),
    ));
    tokio::spawn(run_daily_workflow_loop(
        state.clone(),
        WorkflowKind::Night,
        NaiveTime::from_hms_opt(23, 59, 0).expect("valid time"),
    ));
    tokio::spawn(run_login_check_loop(state));
}

async fn run_daily_workflow_loop(state: Arc<AppState>, kind: WorkflowKind, time: NaiveTime) {
    loop {
        sleep_until(next_daily_instant(time)).await;
        let result = state
            .workflow
            .run_workflow(kind, None, "scheduler", None)
            .await;
        // 后续步骤失败前可能已有数据落库，不能让 24 小时查询缓存继续返回旧数据。
        if let Err(error) = state.query_cache.invalidate_all_shared().await {
            tracing::error!(error = ?error, "跨实例失效查询缓存失败");
        }
        if let Err(error) = result {
            tracing::error!("定时工作流执行失败：kind={kind:?} error={error:?}");
        }
    }
}

async fn run_login_check_loop(state: Arc<AppState>) {
    sleep_until(next_daily_instant(
        NaiveTime::from_hms_opt(0, 30, 0).expect("valid time"),
    ))
    .await;

    loop {
        if let Err(error) = state.workflow.check_all_logins().await {
            tracing::error!("星图登录态巡检失败：{error:?}");
        }

        tokio::time::sleep(Duration::from_secs(60 * 60)).await;
    }
}

fn next_daily_instant(time: NaiveTime) -> Instant {
    instant_from_utc(next_daily_local_time(time).with_timezone(&Utc))
}

fn next_daily_local_time(time: NaiveTime) -> chrono::DateTime<chrono_tz::Tz> {
    let now_utc = Utc::now();
    let now_local = now_utc.with_timezone(&Shanghai);
    let today = now_local.date_naive();
    let mut next_local = Shanghai
        .from_local_datetime(&today.and_time(time))
        .single()
        .expect("Asia/Shanghai local time should be unambiguous");

    if next_local <= now_local {
        let tomorrow = today + ChronoDuration::days(1);
        next_local = Shanghai
            .from_local_datetime(&tomorrow.and_time(time))
            .single()
            .expect("Asia/Shanghai local time should be unambiguous");
    }

    next_local
}

fn instant_from_utc(target: DateTime<Utc>) -> Instant {
    let now = Utc::now();
    let duration = target
        .signed_duration_since(now)
        .to_std()
        .unwrap_or_else(|_| Duration::from_secs(0));
    Instant::now() + duration
}
