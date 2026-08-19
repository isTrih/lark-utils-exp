use crate::server::cache::QueryCache;
use crate::workflow::XingtuWorkflowService;
use sqlx::PgPool;
use std::sync::Arc;

/// Salvo handler 共享状态。
#[derive(Clone)]
pub struct AppState {
    pub pool: PgPool,
    pub workflow: XingtuWorkflowService,
    pub query_cache: QueryCache,
}

impl AppState {
    pub fn new(pool: PgPool, workflow: XingtuWorkflowService) -> Self {
        Self {
            query_cache: QueryCache::new(pool.clone()),
            pool,
            workflow,
        }
    }
}

pub fn state_from_depot(depot: &mut salvo::Depot) -> anyhow::Result<Arc<AppState>> {
    depot
        .obtain::<Arc<AppState>>()
        .map(Clone::clone)
        .map_err(|_| anyhow::anyhow!("服务状态未注入"))
}
