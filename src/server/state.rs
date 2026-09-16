use crate::server::cache::QueryCache;
use crate::server::login::LoginService;
use crate::workflow::{WorkflowKind, WorkflowRunResult, XingtuWorkflowService};
use anyhow::Context;
use sqlx::PgPool;
use std::sync::Arc;

/// Salvo handler 共享状态。
#[derive(Clone)]
pub struct AppState {
    pub pool: PgPool,
    pub workflow: XingtuWorkflowService,
    pub query_cache: QueryCache,
    pub login: LoginService,
    pub recovered_workflow_runs: u64,
}

impl AppState {
    pub fn new(
        pool: PgPool,
        workflow: XingtuWorkflowService,
        login: LoginService,
        recovered_workflow_runs: u64,
    ) -> Self {
        Self {
            query_cache: QueryCache::new(pool.clone()),
            pool,
            workflow,
            login,
            recovered_workflow_runs,
        }
    }

    /// 将 HTTP 触发的完整工作流和缓存失效放入独立任务，客户端断开不会取消后台执行。
    pub async fn run_workflow_independent(
        self: &Arc<Self>,
        kind: WorkflowKind,
        activity_period_id: Option<i64>,
        trigger_source: String,
        request_id: Option<String>,
    ) -> anyhow::Result<WorkflowRunResult> {
        let state = Arc::clone(self);
        tokio::spawn(async move {
            let result = state
                .workflow
                .run_workflow(
                    kind,
                    activity_period_id,
                    &trigger_source,
                    request_id.as_deref(),
                )
                .await;
            crate::server::cache::invalidate_after_write(&state.query_cache, result).await
        })
        .await
        .context("独立工作流任务异常结束")?
    }
}

pub fn state_from_depot(depot: &mut salvo::Depot) -> anyhow::Result<Arc<AppState>> {
    depot
        .obtain::<Arc<AppState>>()
        .map(Clone::clone)
        .map_err(|_| anyhow::anyhow!("服务状态未注入"))
}
