use crate::client::LarkClient;
use crate::lark::im::{
    AuditReviewNoticeConfig, FeishuImClient, SendMessageResult, XingtuLoginNoticeConfig,
};
use crate::lark::message_history::{CardMessageCategory, CardMessageHistoryRepository};
use crate::pipeline::audit_notice::build_pending_audit_info;
use crate::pipeline::audit_result_sync::{AuditResultSyncResult, sync_audit_results_to_database};
use crate::pipeline::workflow::run_activity_sync_workflow;
use crate::workflow_run::WorkflowRunRepository;
use crate::xingtu::account::{
    XingtuAccountRepository, XingtuProjectAccount, XingtuSessionRegistry,
};
use crate::xingtu::activity_config::XingtuActivityConfigRepository;
use crate::xingtu::data_import::{
    PendingImportResult, XingtuDataImportRepository, import_feishu_source_by_id,
    import_pending_feishu_sources,
};
use crate::xingtu::trace::{
    TraceRoundResult, XingtuTraceRunOptions, run_xingtu_trace_once_with_registry,
};
use crate::xingtu::{ExportTaskOptions, XingtuSession};
use anyhow::{Context, anyhow};
use chrono::Utc;
use chrono_tz::Asia::Shanghai;
use salvo::oapi::ToSchema;
use serde::{Deserialize, Serialize};
use serde_json::json;
use sqlx::{Postgres, Transaction};
use std::time::Duration;
use std::{collections::BTreeSet, future::Future, sync::Arc};
use tokio::sync::Mutex;

#[derive(Debug)]
pub(crate) struct WorkflowBusyError;

impl std::fmt::Display for WorkflowBusyError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("已有写工作流正在运行，请稍后重试")
    }
}

impl std::error::Error for WorkflowBusyError {}

struct NotificationGuard<'a> {
    category: &'a str,
    scope: &'a str,
    payload: &'a str,
    cooldown: Duration,
}

/// 工作流类型。
#[derive(Debug, Clone, Copy, Deserialize, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum WorkflowKind {
    Morning,
    Periodic,
    Night,
}

impl WorkflowKind {
    pub fn trigger_type(self) -> &'static str {
        match self {
            Self::Morning => "morning",
            Self::Periodic => "periodic",
            Self::Night => "night",
        }
    }

    pub fn should_notify_audit(self) -> bool {
        matches!(self, Self::Morning)
    }

    pub fn is_daily_final(self) -> bool {
        matches!(self, Self::Night)
    }
}

/// 单次工作流执行结果。
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct WorkflowRunResult {
    pub workflow_run_id: i64,
    pub workflow_kind: WorkflowKind,
    pub processed_activity_period_ids: Vec<i64>,
    pub trace: TraceRoundResult,
    pub pending_import: PendingImportResult,
    pub synced_activities: usize,
    pub audit_result_sync: Option<AuditResultSyncResult>,
    pub audit_notice_sent: bool,
}

/// 单次手动登记专项同步结果。
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct ManualRegistrationSyncResult {
    pub synced_activities: usize,
    pub live_configs: usize,
    pub video_configs: usize,
}

/// 登录态检查结果。
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct LoginCheckResult {
    pub xingtu_account_id: String,
    pub project: String,
    pub valid: bool,
    pub notice_sent: bool,
    pub status_message: Option<String>,
}

/// 单次审核通知检查结果。
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct AuditNoticeRunResult {
    pub audit_notice_sent: bool,
    pub pending_items: Vec<AuditNoticeItemResult>,
}

/// 单期待审核数量。
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct AuditNoticeItemResult {
    pub project_name: String,
    pub period: String,
    pub video_nums: String,
    pub live_nums: String,
    pub audit_table_url: String,
}

/// 调度器和 HTTP 接口共用的工作流服务。
#[derive(Clone)]
pub struct XingtuWorkflowService {
    pub activity_repo: XingtuActivityConfigRepository,
    pub data_import_repo: XingtuDataImportRepository,
    pub account_repo: XingtuAccountRepository,
    pub message_history_repo: CardMessageHistoryRepository,
    pub workflow_run_repo: WorkflowRunRepository,
    pub session_registry: XingtuSessionRegistry,
    pub lark: LarkClient,
    write_lock: Arc<Mutex<()>>,
}

impl XingtuWorkflowService {
    pub fn new(
        activity_repo: XingtuActivityConfigRepository,
        data_import_repo: XingtuDataImportRepository,
        account_repo: XingtuAccountRepository,
        message_history_repo: CardMessageHistoryRepository,
        workflow_run_repo: WorkflowRunRepository,
        session_registry: XingtuSessionRegistry,
        lark: LarkClient,
    ) -> Self {
        Self {
            activity_repo,
            data_import_repo,
            account_repo,
            message_history_repo,
            workflow_run_repo,
            session_registry,
            lark,
            write_lock: Arc::new(Mutex::new(())),
        }
    }

    /// 接收并保存某个项目星图账号的登录态。
    pub async fn upsert_xingtu_session(
        &self,
        xingtu_account_id: &str,
        session: XingtuSession,
    ) -> anyhow::Result<()> {
        self.account_repo
            .upsert_session(xingtu_account_id, &session)
            .await?;
        self.session_registry
            .set_session(xingtu_account_id, session)
            .await?;
        Ok(())
    }

    /// 从数据库恢复已保存的登录态到内存注册表。
    pub async fn restore_sessions_from_db(&self) -> anyhow::Result<usize> {
        let sessions = self.account_repo.list_stored_sessions().await?;
        let count = sessions.len();

        for stored in sessions {
            self.session_registry
                .set_session(&stored.xingtu_account_id, stored.session)
                .await?;
        }

        Ok(count)
    }

    /// 执行每日早、手动周期或每日晚工作流。
    ///
    /// 三种工作流共享拉取、导入和同步步骤；只有每日早会额外通知审核。
    pub async fn run_workflow(
        &self,
        kind: WorkflowKind,
        activity_period_id: Option<i64>,
        trigger_source: &str,
        request_id: Option<&str>,
    ) -> anyhow::Result<WorkflowRunResult> {
        let _guard = self.write_lock.lock().await;
        let (run_id, lock_tx) = self
            .begin_exclusive_run(
                kind.trigger_type(),
                activity_period_id,
                trigger_source,
                request_id,
            )
            .await?;
        let result = self
            .run_workflow_inner(run_id, kind, activity_period_id)
            .await;

        self.finish_serializable_run(run_id, &result).await?;
        lock_tx.commit().await.context("释放跨实例工作流锁失败")?;

        if let Err(error) = &result {
            self.send_workflow_error_notices(kind, error).await;
        }

        result
    }

    async fn run_workflow_inner(
        &self,
        workflow_run_id: i64,
        kind: WorkflowKind,
        activity_period_id: Option<i64>,
    ) -> anyhow::Result<WorkflowRunResult> {
        let pull_started_at = Utc::now();
        let activity_scopes = self
            .activity_repo
            .list_workflow_activity_scopes(pull_started_at, activity_period_id)
            .await
            .context("读取当前自动工作流活动失败")?;
        tracing::info!(
            workflow = kind.trigger_type(),
            requested_activity_period_id = ?activity_period_id,
            activity_scopes = ?activity_scopes,
            "自动工作流已确定项目执行顺序"
        );
        let mut trace = TraceRoundResult::default();
        let mut pending_import = PendingImportResult::default();
        let mut synced_activities = 0;
        let mut audit_result_sync = kind.is_daily_final().then(AuditResultSyncResult::default);
        let mut audit_notice_sent = false;
        let mut processed_activity_period_ids = Vec::new();

        for activity_scope in activity_scopes {
            let activity_period_id = activity_scope.activity_period_id;
            let xingtu_account_id = activity_scope.xingtu_account_id;
            tracing::info!(
                workflow = kind.trigger_type(),
                activity_period_id,
                xingtu_account_id,
                "开始执行项目完整工作流"
            );

            let activity_trace = self
                .tracked_step(workflow_run_id, activity_period_id, "xingtu_export", async {
                    run_xingtu_trace_once_with_registry(
                        &self.activity_repo,
                        &self.session_registry,
                        XingtuTraceRunOptions {
                            export_options: ExportTaskOptions::default(),
                            activity_period_id: Some(activity_period_id),
                            jitter_before_first_export: trace.traced_count > 0,
                            trigger_type: kind.trigger_type().to_string(),
                            is_daily_final: kind.is_daily_final(),
                            pull_started_at,
                            created_by: format!("workflow:{}", kind.trigger_type()),
                        },
                    )
                    .await
                    .with_context(|| {
                        format!(
                            "活动期次 `{activity_period_id}` account_id={xingtu_account_id} 拉取星图源数据链接失败"
                        )
                    })
                })
                .await?;
            trace.traced_count += activity_trace.traced_count;
            trace.skipped_count += activity_trace.skipped_count;
            trace.results.extend(activity_trace.results);

            let activity_import = self
                .tracked_step(workflow_run_id, activity_period_id, "pending_import", async {
                    import_pending_feishu_sources(
                        &self.data_import_repo,
                        &self.lark,
                        200,
                        Some(activity_period_id),
                    )
                    .await
                    .with_context(|| {
                        format!(
                            "活动期次 `{activity_period_id}` account_id={xingtu_account_id} 导入 pending 来源失败"
                        )
                    })
                })
                .await?;
            merge_pending_import_result(&mut pending_import, activity_import);

            synced_activities += self
                .tracked_step(workflow_run_id, activity_period_id, "table_sync", async {
                    let activity_configs = self
                        .activity_repo
                        .list_sync_activity_configs(
                            self.data_import_repo.clone(),
                            pull_started_at,
                            Some(activity_period_id),
                        )
                        .await
                        .with_context(|| {
                            format!(
                                "活动期次 `{activity_period_id}` account_id={xingtu_account_id} 读取同步配置失败"
                            )
                        })?;
                    let count = activity_configs.len();
                    run_activity_sync_workflow(activity_configs)
                        .await
                        .with_context(|| {
                            format!(
                                "活动期次 `{activity_period_id}` account_id={xingtu_account_id} 同步数据到飞书多维表失败"
                            )
                        })?;
                    Ok(count)
                })
                .await?;

            if let Some(total) = audit_result_sync.as_mut() {
                tracing::info!(
                    workflow = kind.trigger_type(),
                    activity_period_id,
                    "开始从审核表同步审核结果"
                );
                let result = self
                    .tracked_step(
                        workflow_run_id,
                        activity_period_id,
                        "audit_result_sync",
                        async {
                            self.sync_audit_results_from_db(Some(activity_period_id))
                                .await
                                .with_context(|| {
                                    format!(
                                        "活动期次 `{activity_period_id}` account_id={xingtu_account_id} 从审核表同步结果失败"
                                    )
                                })
                        },
                    )
                    .await?;
                total.tables_processed += result.tables_processed;
                total.records_read += result.records_read;
                total.reviewed_records += result.reviewed_records;
                total.database_rows_updated += result.database_rows_updated;
            }

            if kind.should_notify_audit() {
                audit_notice_sent |= self
                    .tracked_step(
                        workflow_run_id,
                        activity_period_id,
                        "audit_notice",
                        async {
                            self.run_audit_notice_from_db(Some(activity_period_id), true)
                                .await
                                .with_context(|| {
                                    format!(
                                        "活动期次 `{activity_period_id}` account_id={xingtu_account_id} 发送审核通知失败"
                                    )
                                })
                        },
                    )
                    .await?
                    .audit_notice_sent;
            }

            processed_activity_period_ids.push(activity_period_id);
            tracing::info!(
                workflow = kind.trigger_type(),
                activity_period_id,
                xingtu_account_id,
                "项目完整工作流执行完成"
            );
        }

        Ok(WorkflowRunResult {
            workflow_run_id,
            workflow_kind: kind,
            processed_activity_period_ids,
            trace,
            pending_import,
            synced_activities,
            audit_result_sync,
            audit_notice_sent,
        })
    }

    /// 只导入已有 pending 来源。
    pub async fn import_pending_sources(
        &self,
        limit: i64,
        activity_period_id: Option<i64>,
        trigger_source: &str,
        request_id: Option<&str>,
    ) -> anyhow::Result<PendingImportResult> {
        let _guard = self.write_lock.lock().await;
        let (run_id, lock_tx) = self
            .begin_exclusive_run(
                "pending_import",
                activity_period_id,
                trigger_source,
                request_id,
            )
            .await?;
        let result = self
            .tracked_optional_step(run_id, activity_period_id, "pending_import", async {
                import_pending_feishu_sources(
                    &self.data_import_repo,
                    &self.lark,
                    limit,
                    activity_period_id,
                )
                .await
            })
            .await;
        self.finish_serializable_run(run_id, &result).await?;
        lock_tx.commit().await?;
        result
    }

    /// 管理端对单个 failed/dead-letter 来源执行精确重试。
    pub async fn retry_failed_source_only(
        &self,
        feishu_source_id: i64,
        trigger_source: &str,
        request_id: Option<&str>,
    ) -> anyhow::Result<PendingImportResult> {
        let _guard = self.write_lock.lock().await;
        let (run_id, lock_tx) = self
            .begin_exclusive_run("source_retry", None, trigger_source, request_id)
            .await?;
        let result = self
            .tracked_optional_step(run_id, None, "source_retry", async {
                if !self
                    .data_import_repo
                    .retry_failed_source(feishu_source_id)
                    .await?
                {
                    return Err(anyhow!("未找到可重试来源：{feishu_source_id}"));
                }
                import_feishu_source_by_id(&self.data_import_repo, &self.lark, feishu_source_id)
                    .await
            })
            .await;
        self.finish_serializable_run(run_id, &result).await?;
        lock_tx.commit().await?;
        result
    }

    /// 只统计待审核数量并发送审核通知，不执行拉取、导入或同步。
    pub async fn run_audit_notice_only(
        &self,
        activity_period_id: Option<i64>,
        trigger_source: &str,
        request_id: Option<&str>,
    ) -> anyhow::Result<AuditNoticeRunResult> {
        let _guard = self.write_lock.lock().await;
        let (run_id, lock_tx) = self
            .begin_exclusive_run(
                "audit_notice",
                activity_period_id,
                trigger_source,
                request_id,
            )
            .await?;
        let result = self
            .tracked_optional_step(run_id, activity_period_id, "audit_notice", async {
                self.run_audit_notice_from_db(activity_period_id, activity_period_id.is_none())
                    .await
            })
            .await;
        self.finish_serializable_run(run_id, &result).await?;
        lock_tx.commit().await?;
        result
    }

    /// 只从当前启用活动的审核表回写审核结果，不执行其他同步步骤。
    pub async fn sync_audit_results_only(
        &self,
        activity_period_id: Option<i64>,
        trigger_source: &str,
        request_id: Option<&str>,
    ) -> anyhow::Result<AuditResultSyncResult> {
        let _guard = self.write_lock.lock().await;
        let (run_id, lock_tx) = self
            .begin_exclusive_run(
                "audit_result_sync",
                activity_period_id,
                trigger_source,
                request_id,
            )
            .await?;
        let result = self
            .tracked_optional_step(run_id, activity_period_id, "audit_result_sync", async {
                self.sync_audit_results_from_db(activity_period_id).await
            })
            .await;
        self.finish_serializable_run(run_id, &result).await?;
        lock_tx.commit().await?;
        result
    }

    /// 只同步直播/视频手动登记数据，不执行星图拉取、来源导入或审核通知。
    pub async fn sync_manual_registrations_only(
        &self,
        activity_period_id: Option<i64>,
        trigger_source: &str,
        request_id: Option<&str>,
    ) -> anyhow::Result<ManualRegistrationSyncResult> {
        let _guard = self.write_lock.lock().await;
        let (run_id, lock_tx) = self
            .begin_exclusive_run(
                "manual_sync",
                activity_period_id,
                trigger_source,
                request_id,
            )
            .await?;
        let result = self
            .tracked_optional_step(run_id, activity_period_id, "manual_sync", async {
                self.sync_manual_registrations_inner(activity_period_id)
                    .await
            })
            .await;
        self.finish_serializable_run(run_id, &result).await?;
        lock_tx.commit().await?;
        result
    }

    async fn sync_manual_registrations_inner(
        &self,
        activity_period_id: Option<i64>,
    ) -> anyhow::Result<ManualRegistrationSyncResult> {
        let activity_configs = self
            .activity_repo
            .list_manual_sync_activity_configs(self.data_import_repo.clone(), activity_period_id)
            .await
            .context("读取手动登记同步配置失败")?;
        let result = ManualRegistrationSyncResult {
            synced_activities: activity_configs.len(),
            live_configs: activity_configs
                .iter()
                .filter(|activity| activity.live.is_some())
                .count(),
            video_configs: activity_configs
                .iter()
                .filter(|activity| activity.video.is_some())
                .count(),
        };

        tracing::info!(
            activities = result.synced_activities,
            live_configs = result.live_configs,
            video_configs = result.video_configs,
            "开始执行手动登记专项同步"
        );
        run_activity_sync_workflow(activity_configs)
            .await
            .context("同步手动登记数据失败")?;
        tracing::info!("手动登记专项同步完成");

        Ok(result)
    }

    async fn tracked_step<T, F>(
        &self,
        run_id: i64,
        activity_period_id: i64,
        step_name: &str,
        operation: F,
    ) -> anyhow::Result<T>
    where
        T: Serialize,
        F: Future<Output = anyhow::Result<T>>,
    {
        self.tracked_optional_step(run_id, Some(activity_period_id), step_name, operation)
            .await
    }

    async fn tracked_optional_step<T, F>(
        &self,
        run_id: i64,
        activity_period_id: Option<i64>,
        step_name: &str,
        operation: F,
    ) -> anyhow::Result<T>
    where
        T: Serialize,
        F: Future<Output = anyhow::Result<T>>,
    {
        let step_id = self
            .workflow_run_repo
            .start_step(run_id, activity_period_id, step_name, None)
            .await?;
        let result = operation.await;
        match &result {
            Ok(value) => {
                self.workflow_run_repo
                    .finish_step_success(step_id, json!({ "result": value }))
                    .await?;
            }
            Err(error) => {
                self.workflow_run_repo
                    .finish_step_failure(step_id, &format!("{error:#}"))
                    .await?;
            }
        }
        result
    }

    async fn begin_exclusive_run(
        &self,
        kind: &str,
        activity_period_id: Option<i64>,
        trigger_source: &str,
        request_id: Option<&str>,
    ) -> anyhow::Result<(i64, Transaction<'static, Postgres>)> {
        let run_id = self
            .workflow_run_repo
            .start_run(kind, activity_period_id, trigger_source, request_id)
            .await?;
        let lock_tx = match self.workflow_run_repo.try_acquire_global_lock().await {
            Ok(lock_tx) => lock_tx,
            Err(error) => {
                if let Err(finish_error) = self
                    .workflow_run_repo
                    .finish_run_failure(
                        run_id,
                        "failed",
                        "lock_failed",
                        &format!("获取跨实例工作流锁失败：{error}"),
                    )
                    .await
                {
                    tracing::error!(run_id, error = ?finish_error, "记录工作流锁故障失败");
                }
                return Err(error).context("获取跨实例工作流锁失败");
            }
        };
        let Some(lock_tx) = lock_tx else {
            self.workflow_run_repo
                .finish_run_failure(run_id, "blocked", "workflow_busy", "已有写工作流正在运行")
                .await?;
            return Err(WorkflowBusyError.into());
        };
        Ok((run_id, lock_tx))
    }

    async fn finish_serializable_run<T>(
        &self,
        run_id: i64,
        result: &anyhow::Result<T>,
    ) -> anyhow::Result<()>
    where
        T: Serialize,
    {
        match result {
            Ok(value) => {
                self.workflow_run_repo
                    .finish_run_success(run_id, json!({ "result": value }))
                    .await
            }
            Err(error) => {
                self.workflow_run_repo
                    .finish_run_failure(run_id, "failed", "operation_failed", &format!("{error:#}"))
                    .await
            }
        }
    }

    /// 检查单个星图账号登录态。
    pub async fn check_single_login(
        &self,
        account: &XingtuProjectAccount,
    ) -> anyhow::Result<LoginCheckResult> {
        let client = self
            .session_registry
            .get_client(&account.xingtu_account_id)
            .await?;
        let check = client.check_session().await;

        match check {
            Ok(result) => {
                self.account_repo
                    .update_session_check_status(
                        &account.xingtu_account_id,
                        result.valid,
                        result.status_message.as_deref(),
                    )
                    .await?;

                let notice_sent = if result.valid {
                    if let Err(error) = self
                        .workflow_run_repo
                        .clear_notification("login_invalid", &account.xingtu_account_id)
                        .await
                    {
                        tracing::error!(
                            account_id = %account.xingtu_account_id,
                            error = ?error,
                            "清理登录失效通知冷却状态失败"
                        );
                    }
                    false
                } else {
                    self.send_login_invalid_notice(account, result.status_message.as_deref())
                        .await
                        .is_some()
                };

                Ok(LoginCheckResult {
                    xingtu_account_id: account.xingtu_account_id.clone(),
                    project: account.project.clone(),
                    valid: result.valid,
                    notice_sent,
                    status_message: result.status_message,
                })
            }
            Err(error) => {
                let message = format!("{error:?}");
                self.account_repo
                    .update_session_check_status(&account.xingtu_account_id, false, Some(&message))
                    .await?;
                let notice_sent = self
                    .send_login_invalid_notice(account, Some(&message))
                    .await
                    .is_some();

                Ok(LoginCheckResult {
                    xingtu_account_id: account.xingtu_account_id.clone(),
                    project: account.project.clone(),
                    valid: false,
                    notice_sent,
                    status_message: Some(message),
                })
            }
        }
    }

    /// 巡检所有启用的星图账号登录态。
    pub async fn check_all_logins(&self) -> anyhow::Result<Vec<LoginCheckResult>> {
        let accounts = self.account_repo.list_login_check_accounts().await?;
        let mut results = Vec::with_capacity(accounts.len());

        for account in accounts {
            results.push(self.check_single_login(&account).await?);
        }

        Ok(results)
    }

    async fn send_login_invalid_notice(
        &self,
        account: &XingtuProjectAccount,
        status_message: Option<&str>,
    ) -> Option<SendMessageResult> {
        let error_detail = login_invalid_error_detail(&account.xingtu_account_id, status_message);
        self.send_error_notice(
            account,
            "星图登陆状态失效",
            &error_detail,
            NotificationGuard {
                category: "login_invalid",
                scope: &account.xingtu_account_id,
                payload: status_message.unwrap_or("invalid"),
                cooldown: Duration::from_secs(12 * 60 * 60),
            },
        )
        .await
    }

    async fn send_workflow_error_notices(&self, kind: WorkflowKind, error: &anyhow::Error) {
        let accounts = match self.account_repo.list_accounts().await {
            Ok(accounts) => accounts,
            Err(account_error) => {
                tracing::error!(
                    "工作流失败后查询通知账号失败：workflow={} error={account_error:?}",
                    kind.trigger_type()
                );
                return;
            }
        };
        let error_chain = format!("{error:#}");
        let error_type = classify_workflow_error(&error_chain);
        let notice_accounts = select_workflow_error_notice_accounts(&accounts, &error_chain);

        for account in notice_accounts {
            let error_detail =
                workflow_error_detail(&account.xingtu_account_id, kind, error_type, error);
            if self
                .send_error_notice(
                    account,
                    error_type,
                    &error_detail,
                    NotificationGuard {
                        category: "workflow_error",
                        scope: &format!("{}:{}", account.xingtu_account_id, kind.trigger_type()),
                        payload: error_type,
                        cooldown: Duration::from_secs(60 * 60),
                    },
                )
                .await
                .is_none()
            {
                tracing::error!(
                    "发送工作流错误通知失败：workflow={} account_id={}",
                    kind.trigger_type(),
                    account.xingtu_account_id
                );
            }
        }
    }

    async fn send_error_notice(
        &self,
        account: &XingtuProjectAccount,
        error_type: &str,
        error_detail: &str,
        guard: NotificationGuard<'_>,
    ) -> Option<SendMessageResult> {
        let receiver = account.message_receiver().ok()?;
        let claimed = match self
            .workflow_run_repo
            .claim_notification(guard.category, guard.scope, guard.payload, guard.cooldown)
            .await
        {
            Ok(claimed) => claimed,
            Err(error) => {
                tracing::error!(error = ?error, guard_category = guard.category, guard_scope = guard.scope, "申请通知去重租约失败");
                return None;
            }
        };
        if !claimed {
            tracing::info!(
                guard_category = guard.category,
                guard_scope = guard.scope,
                "相同业务通知仍在冷却窗口，跳过发送"
            );
            return None;
        }
        let notice = XingtuLoginNoticeConfig {
            template_id: account.login_notice_card_template_id.clone(),
            ops_ids: account.ops_ids_text(),
            project_name: account.project_name(),
            error_type: error_type.to_string(),
            error_detail: error_detail.to_string(),
        };

        match FeishuImClient::new(&self.lark)
            .send_xingtu_login_notice(&receiver, &notice)
            .await
        {
            Ok(result) => {
                let project_name = account.project_name();
                self.message_history_repo
                    .record_sent_message(
                        result.message_id.as_deref(),
                        CardMessageCategory::ErrorLog,
                        format!("{project_name}：{error_type}"),
                        &receiver,
                        Some(&project_name),
                        None,
                    )
                    .await;
                if let Err(error) = self
                    .workflow_run_repo
                    .mark_notification_sent(guard.category, guard.scope)
                    .await
                {
                    tracing::error!(error = ?error, guard_category = guard.category, guard_scope = guard.scope, "更新通知去重状态失败");
                }
                Some(result)
            }
            Err(error) => {
                if let Err(release_error) = self
                    .workflow_run_repo
                    .release_notification(guard.category, guard.scope)
                    .await
                {
                    tracing::error!(error = ?release_error, guard_category = guard.category, guard_scope = guard.scope, "释放通知去重租约失败");
                }
                tracing::error!(
                    "发送星图/工作流错误通知失败：account_id={} error_type={} error={error:?}",
                    account.xingtu_account_id,
                    error_type
                );
                None
            }
        }
    }

    async fn sync_audit_results_from_db(
        &self,
        activity_period_id: Option<i64>,
    ) -> anyhow::Result<AuditResultSyncResult> {
        let configs = self
            .activity_repo
            .list_audit_result_sync_configs(activity_period_id)
            .await?;
        sync_audit_results_to_database(&self.lark, &self.data_import_repo, configs).await
    }

    async fn run_audit_notice_from_db(
        &self,
        activity_period_id: Option<i64>,
        respect_tracking_window: bool,
    ) -> anyhow::Result<AuditNoticeRunResult> {
        let notice_configs = self
            .activity_repo
            .build_audit_notice_configs_from_db(
                respect_tracking_window.then(Utc::now),
                activity_period_id,
            )
            .await?;

        if notice_configs.is_empty() {
            return Ok(AuditNoticeRunResult {
                audit_notice_sent: false,
                pending_items: Vec::new(),
            });
        }

        let mut pending_items = Vec::new();
        let mut audit_notice_sent = false;

        for notice_config in notice_configs {
            let project_name = notice_config.project_name.clone();
            let audit_info = build_pending_audit_info(&self.lark, &notice_config)
                .await
                .with_context(|| format!("构建项目 `{project_name}` 审核通知卡片参数失败"))?;
            pending_items.extend(audit_info.iter().map(|item| AuditNoticeItemResult {
                project_name: project_name.clone(),
                period: item.period.clone(),
                video_nums: item.video_nums.clone(),
                live_nums: item.live_nums.clone(),
                audit_table_url: item.audit_table_url.clone(),
            }));

            if audit_info.is_empty() {
                tracing::info!(project = %project_name, "项目没有待审核记录，跳过审核通知");
                continue;
            }
            if notice_config.auditor_ids.trim().is_empty() {
                tracing::info!(project = %project_name, "项目没有启用的审核人，跳过审核通知");
                continue;
            }

            let summary = audit_notice_summary(&project_name, &audit_info);
            let guard_scope = format!("{}:{}", project_name, notice_config.receiver.receive_id);
            let claimed = self
                .workflow_run_repo
                .claim_notification(
                    "audit",
                    &guard_scope,
                    &summary,
                    Duration::from_secs(12 * 60 * 60),
                )
                .await?;
            if !claimed {
                tracing::info!(project = %project_name, "待审核集合未变化且仍在冷却窗口，跳过重复通知");
                continue;
            }
            let card = AuditReviewNoticeConfig::with_template(
                notice_config.card_template_id.trim(),
                notice_config.auditor_ids.trim(),
                notice_config.project_name.trim(),
                audit_info,
            );
            let result = FeishuImClient::new(&self.lark)
                .send_audit_review_notice(&notice_config.receiver, &card)
                .await
                .with_context(|| format!("发送项目 `{project_name}` 审核通知失败"));
            let result = match result {
                Ok(result) => result,
                Err(error) => {
                    self.workflow_run_repo
                        .release_notification("audit", &guard_scope)
                        .await?;
                    return Err(error);
                }
            };
            tracing::info!(
                project = %project_name,
                message_id = ?result.message_id,
                "项目审核通知发送完成"
            );
            self.message_history_repo
                .record_sent_message(
                    result.message_id.as_deref(),
                    CardMessageCategory::Audit,
                    summary,
                    &notice_config.receiver,
                    Some(&project_name),
                    None,
                )
                .await;
            self.workflow_run_repo
                .mark_notification_sent("audit", &guard_scope)
                .await?;
            audit_notice_sent = true;
        }

        Ok(AuditNoticeRunResult {
            audit_notice_sent,
            pending_items,
        })
    }
}

fn audit_notice_summary(
    project_name: &str,
    audit_info: &[crate::lark::im::AuditInfoItem],
) -> String {
    let periods = audit_info
        .iter()
        .map(|item| {
            format!(
                "{}（视频 {}，直播 {}）",
                item.period, item.video_nums, item.live_nums
            )
        })
        .collect::<Vec<_>>()
        .join("；");
    format!("{project_name} 审核通知：{periods}")
}

fn merge_pending_import_result(total: &mut PendingImportResult, current: PendingImportResult) {
    total.discovered_sources += current.discovered_sources;
    total.imported_sources += current.imported_sources;
    total.partial_sources += current.partial_sources;
    total.failed_sources += current.failed_sources;
    total.dead_lettered_sources += current.dead_lettered_sources;
    total.persisted_rows += current.persisted_rows;
    total.quarantined_rows += current.quarantined_rows;
    total.failures.extend(current.failures);
}

fn select_workflow_error_notice_accounts<'a>(
    accounts: &'a [XingtuProjectAccount],
    error_chain: &str,
) -> Vec<&'a XingtuProjectAccount> {
    let has_account_match = accounts
        .iter()
        .any(|account| error_chain.contains(&account.xingtu_account_id));
    let mut receiver_keys = BTreeSet::new();

    accounts
        .iter()
        .filter(|account| !has_account_match || error_chain.contains(&account.xingtu_account_id))
        .filter(|account| {
            receiver_keys.insert((
                account.receive_id_type.as_str(),
                account.receive_id.as_str(),
            ))
        })
        .collect()
}

fn login_invalid_error_detail(xingtu_account_id: &str, status_message: Option<&str>) -> String {
    let mut lines = vec![
        "星图登陆态失效，请及时更新星图登陆态。".to_string(),
        format!("账号ID:{xingtu_account_id}"),
    ];

    if let Some(message) = status_message
        .map(str::trim)
        .filter(|message| !message.is_empty())
    {
        lines.push(format!("错误信息:{message}"));
    }

    lines.push(beijing_timestamp());
    lines.join("\n")
}

fn classify_workflow_error(error_chain: &str) -> &'static str {
    let lowercase = error_chain.to_ascii_lowercase();
    if error_chain.contains("未登录")
        || error_chain.contains("登录态")
        || error_chain.contains("登陆态")
        || lowercase.contains("cookie")
        || lowercase.contains("csrf")
    {
        return "星图登陆状态失效";
    }

    if error_chain.contains("星图接口失败")
        || error_chain.contains("星图长任务失败")
        || error_chain.contains("等待星图导出结果超时")
        || error_chain.contains("星图任务导出失败")
        || lowercase.contains("xingtu.cn")
    {
        return "星图数据拉取失败";
    }

    "工作流执行失败"
}

fn workflow_error_detail(
    xingtu_account_id: &str,
    kind: WorkflowKind,
    error_type: &str,
    error: &anyhow::Error,
) -> String {
    let introduction = match error_type {
        "星图登陆状态失效" => "星图登陆态失效，请及时更新星图登陆态。",
        "星图数据拉取失败" => "星图数据拉取失败，请及时检查星图返回信息。",
        _ => "自动化工作流执行失败，请及时检查。",
    };
    let error_chain = format!("{error:#}");
    let root_cause = error
        .chain()
        .last()
        .map(ToString::to_string)
        .unwrap_or_else(|| error.to_string());
    let mut lines = vec![
        introduction.to_string(),
        format!("工作流:{}", kind.trigger_type()),
        format!("错误信息:{root_cause}"),
    ];

    if error_chain != root_cause {
        lines.push(format!("错误上下文:{error_chain}"));
    }

    lines.push(format!("账号ID:{xingtu_account_id}"));
    lines.push(beijing_timestamp());
    lines.join("\n")
}

fn beijing_timestamp() -> String {
    Utc::now()
        .with_timezone(&Shanghai)
        .format("%Y:%m:%d %H:%M:%S")
        .to_string()
}

pub fn parse_workflow_kind(value: &str) -> anyhow::Result<WorkflowKind> {
    match value.trim().to_ascii_lowercase().as_str() {
        "morning" => Ok(WorkflowKind::Morning),
        "periodic" => Ok(WorkflowKind::Periodic),
        "night" => Ok(WorkflowKind::Night),
        other => Err(anyhow!("未知工作流类型：{other}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn workflow_account(
        xingtu_account_id: &str,
        project: &str,
        receive_id: &str,
    ) -> XingtuProjectAccount {
        XingtuProjectAccount {
            xingtu_account_id: xingtu_account_id.to_string(),
            project_id: 1,
            project: project.to_string(),
            project_display_name: project.to_string(),
            display_name: None,
            receive_id_type: "chat_id".to_string(),
            receive_id: receive_id.to_string(),
            ops_ids: Vec::new(),
            login_notice_card_template_id: "template".to_string(),
            login_check_enabled: true,
            session_status: "unknown".to_string(),
            last_checked_at: None,
            last_valid_at: None,
            last_invalid_at: None,
            last_error: None,
        }
    }

    #[test]
    fn workflow_error_notice_targets_the_activity_account() {
        let accounts = vec![
            workflow_account("account-a", "project-a", "shared-chat"),
            workflow_account("account-b", "project-b", "shared-chat"),
        ];

        let selected = select_workflow_error_notice_accounts(
            &accounts,
            "活动期次 `2` account_id=account-b 同步数据失败",
        );

        assert_eq!(selected.len(), 1);
        assert_eq!(selected[0].xingtu_account_id, "account-b");
    }

    #[test]
    fn workflow_error_notice_deduplicates_a_shared_receiver() {
        let accounts = vec![
            workflow_account("account-a", "project-a", "shared-chat"),
            workflow_account("account-b", "project-b", "shared-chat"),
        ];

        let selected = select_workflow_error_notice_accounts(&accounts, "读取工作流活动范围失败");

        assert_eq!(selected.len(), 1);
        assert_eq!(selected[0].receive_id, "shared-chat");
    }

    #[test]
    fn workflow_error_classifier_distinguishes_xingtu_and_login_failures() {
        assert_eq!(
            classify_workflow_error("星图任务导出失败：123: 星图长任务失败：reason=文档创建失败"),
            "星图数据拉取失败"
        );
        assert_eq!(
            classify_workflow_error("星图接口失败：status_code=1001 status_message=未登录"),
            "星图登陆状态失效"
        );
        assert_eq!(
            classify_workflow_error("同步活动数据到飞书多维表失败"),
            "工作流执行失败"
        );
    }

    #[test]
    fn audit_history_summary_contains_project_period_and_counts() {
        let summary = audit_notice_summary(
            "ROK",
            &[crate::lark::im::AuditInfoItem::new(
                "2026年8月第十五期",
                12,
                3,
                "https://example.feishu.cn/base/audit",
            )],
        );

        assert_eq!(
            summary,
            "ROK 审核通知：2026年8月第十五期（视频 12，直播 3）"
        );
    }

    #[test]
    fn login_invalid_detail_contains_account_reason_and_beijing_time() {
        let detail = login_invalid_error_detail("demo-xingtu-account", Some("未登录"));
        let lines = detail.lines().collect::<Vec<_>>();

        assert_eq!(lines[0], "星图登陆态失效，请及时更新星图登陆态。");
        assert_eq!(lines[1], "账号ID:demo-xingtu-account");
        assert_eq!(lines[2], "错误信息:未登录");
        assert_eq!(lines[3].len(), 19);
        assert_eq!(&lines[3][4..5], ":");
        assert_eq!(&lines[3][7..8], ":");
        assert_eq!(&lines[3][10..11], " ");
    }

    #[test]
    fn workflow_error_detail_keeps_external_xingtu_reason() {
        let error = anyhow!("星图长任务失败：status=4 reason=文档创建失败")
            .context("拉取星图内容失败：project=ROK account_id=demo-xingtu-account");
        let detail = workflow_error_detail(
            "demo-xingtu-account",
            WorkflowKind::Periodic,
            "星图数据拉取失败",
            &error,
        );

        assert!(detail.contains("错误信息:星图长任务失败：status=4 reason=文档创建失败"));
        assert!(detail.contains("project=ROK"));
        assert!(detail.contains("账号ID:demo-xingtu-account"));
    }
}
