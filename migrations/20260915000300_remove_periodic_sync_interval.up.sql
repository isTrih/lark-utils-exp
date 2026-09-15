ALTER TABLE xingtu_activity_period
    DROP COLUMN periodic_sync_interval_hours;

COMMENT ON COLUMN xingtu_activity_period.periodic_sync_enabled IS
    '是否启用北京时间 12:00、18:00 固定时点同步工作流：拉取并同步，不发送审核通知';
