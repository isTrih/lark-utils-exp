use crate::server::state::AppState;
use crate::workflow::WorkflowKind;
use chrono::{DateTime, Duration as ChronoDuration, NaiveTime, TimeZone, Utc};
use chrono_tz::Asia::Shanghai;
use std::{sync::Arc, time::Duration};
use tokio::time::{Instant, sleep_until};

const PERIODIC_TIMES: [NaiveTime; 2] = [
    NaiveTime::from_hms_opt(12, 0, 0).expect("valid time"),
    NaiveTime::from_hms_opt(18, 0, 0).expect("valid time"),
];
const MORNING_TIME: NaiveTime = NaiveTime::from_hms_opt(9, 0, 0).expect("valid time");
const NIGHT_TIME: NaiveTime = NaiveTime::from_hms_opt(23, 59, 0).expect("valid time");
const LOGIN_CHECK_TIME: NaiveTime = NaiveTime::from_hms_opt(0, 30, 0).expect("valid time");
const ORPHAN_RECOVERY_INTERVAL: Duration = Duration::from_secs(5 * 60);
const ORPHAN_MINIMUM_AGE: Duration = Duration::from_secs(2 * 60);

/// 启动内置调度器。
///
/// 生产环境也可以改成由企业后端调度器调用 HTTP 手动执行接口；
/// 这里先落地服务自带调度，方便独立运行和调试。
pub fn spawn_scheduler(state: Arc<AppState>) {
    tracing::info!(
        "内置调度器已启动：periodic 下一次执行 [{}, {}]，morning 下一次执行 {}，night 下一次执行 {}，登录态巡检下一次执行 {}",
        next_daily_local_time(PERIODIC_TIMES[0]),
        next_daily_local_time(PERIODIC_TIMES[1]),
        next_daily_local_time(MORNING_TIME),
        next_daily_local_time(NIGHT_TIME),
        next_daily_local_time(LOGIN_CHECK_TIME),
    );
    tokio::spawn(run_orphan_recovery_loop(
        state.clone(),
        state.recovered_workflow_runs > 0,
    ));
    for time in PERIODIC_TIMES {
        tokio::spawn(run_daily_workflow_loop(
            state.clone(),
            WorkflowKind::Periodic,
            time,
        ));
    }
    tokio::spawn(run_daily_workflow_loop(
        state.clone(),
        WorkflowKind::Morning,
        MORNING_TIME,
    ));
    tokio::spawn(run_daily_workflow_loop(
        state.clone(),
        WorkflowKind::Night,
        NIGHT_TIME,
    ));
    tokio::spawn(run_login_check_loop(state));
}

async fn run_orphan_recovery_loop(state: Arc<AppState>, mut catch_up_required: bool) {
    loop {
        if catch_up_required {
            tracing::warn!("检测到异常中断的工作流，开始执行一次周期同步补偿");
            let result = state
                .workflow
                .run_workflow(WorkflowKind::Periodic, None, "orphan_recovery", None)
                .await;
            if let Err(error) = state.query_cache.invalidate_all_shared().await {
                tracing::error!(error = ?error, "工作流补偿后跨实例失效查询缓存失败");
            }
            match result {
                Ok(result) if result.failed_activities.is_empty() => {
                    tracing::info!(
                        processed_activity_period_ids = ?result.processed_activity_period_ids,
                        "异常中断工作流的数据同步补偿成功"
                    );
                }
                Ok(result) => {
                    tracing::error!(
                        processed_activity_period_ids = ?result.processed_activity_period_ids,
                        failed_activities = ?result.failed_activities,
                        "异常中断工作流的数据同步补偿存在项目失败"
                    );
                }
                Err(error) => {
                    tracing::error!(error = ?error, "异常中断工作流的数据同步补偿失败");
                }
            }
            catch_up_required = false;
        }

        tokio::time::sleep(ORPHAN_RECOVERY_INTERVAL).await;
        match state
            .workflow
            .workflow_run_repo
            .reconcile_orphaned_runs_if_idle(ORPHAN_MINIMUM_AGE)
            .await
        {
            Ok(0) => {}
            Ok(recovered_runs) => {
                tracing::warn!(recovered_runs, "已回收异常中断的工作流，准备补偿同步");
                catch_up_required = true;
            }
            Err(error) => {
                tracing::error!(error = ?error, "巡检异常中断工作流失败");
            }
        }
    }
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
        match result {
            Ok(result) if result.failed_activities.is_empty() => {
                tracing::info!(
                    kind = ?kind,
                    processed_activity_period_ids = ?result.processed_activity_period_ids,
                    "定时工作流全部项目执行成功"
                );
            }
            Ok(result) => {
                tracing::error!(
                    kind = ?kind,
                    processed_activity_period_ids = ?result.processed_activity_period_ids,
                    failed_activities = ?result.failed_activities,
                    "定时工作流存在项目执行失败"
                );
            }
            Err(error) => {
                tracing::error!("定时工作流执行失败：kind={kind:?} error={error:?}");
            }
        }
    }
}

async fn run_login_check_loop(state: Arc<AppState>) {
    sleep_until(next_daily_instant(LOGIN_CHECK_TIME)).await;

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn built_in_workflow_times_match_business_schedule() {
        assert_eq!(
            PERIODIC_TIMES,
            [
                NaiveTime::from_hms_opt(12, 0, 0).unwrap(),
                NaiveTime::from_hms_opt(18, 0, 0).unwrap(),
            ]
        );
        assert_eq!(MORNING_TIME, NaiveTime::from_hms_opt(9, 0, 0).unwrap());
        assert_eq!(NIGHT_TIME, NaiveTime::from_hms_opt(23, 59, 0).unwrap());
    }
}
