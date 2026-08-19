DROP TRIGGER IF EXISTS trg_xingtu_login_session_updated_at ON xingtu_login_session;
DROP TRIGGER IF EXISTS trg_xingtu_project_account_updated_at ON xingtu_project_account;

COMMENT ON COLUMN xingtu_activity_content_config.trace_enabled IS '是否启用星图定时拉取';

ALTER TABLE xingtu_feishu_source
    DROP COLUMN IF EXISTS pull_started_at;

ALTER TABLE xingtu_activity_period
    DROP COLUMN IF EXISTS xingtu_account_id;

DROP TABLE IF EXISTS xingtu_login_session;
DROP TABLE IF EXISTS xingtu_project_account;

DROP TYPE IF EXISTS xingtu_session_status;
