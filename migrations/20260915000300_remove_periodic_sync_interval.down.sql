ALTER TABLE xingtu_activity_period
    ADD COLUMN periodic_sync_interval_hours INTEGER NOT NULL DEFAULT 2,
    ADD CONSTRAINT chk_xingtu_activity_period_periodic_interval
        CHECK (periodic_sync_interval_hours > 0);

COMMENT ON COLUMN xingtu_activity_period.periodic_sync_enabled IS
    '是否启用周期同步工作流：拉取并同步，不发送审核通知';
COMMENT ON COLUMN xingtu_activity_period.periodic_sync_interval_hours IS
    '周期同步间隔小时数，调度器可按此生成定时任务';
