CREATE TABLE workflow_run (
    workflow_run_id BIGSERIAL PRIMARY KEY,
    workflow_kind TEXT NOT NULL,
    scope_activity_period_id BIGINT REFERENCES xingtu_activity_period(activity_period_id),
    trigger_source TEXT NOT NULL,
    status TEXT NOT NULL DEFAULT 'running',
    started_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    finished_at TIMESTAMPTZ,
    summary JSONB NOT NULL DEFAULT '{}'::jsonb,
    error_code TEXT,
    error_message TEXT,
    request_id TEXT,
    CONSTRAINT chk_workflow_run_kind_not_blank CHECK (btrim(workflow_kind) <> ''),
    CONSTRAINT chk_workflow_run_trigger_not_blank CHECK (btrim(trigger_source) <> ''),
    CONSTRAINT chk_workflow_run_status CHECK (status IN ('running', 'succeeded', 'failed', 'blocked')),
    CONSTRAINT chk_workflow_run_summary_object CHECK (jsonb_typeof(summary) = 'object')
);
COMMENT ON TABLE workflow_run IS '工作流运行台账，用于跨进程排他、运行历史和故障定位';
CREATE INDEX idx_workflow_run_started_at ON workflow_run (started_at DESC);
CREATE INDEX idx_workflow_run_scope_status ON workflow_run (scope_activity_period_id, status, started_at DESC);

CREATE TABLE workflow_step (
    workflow_step_id BIGSERIAL PRIMARY KEY,
    workflow_run_id BIGINT NOT NULL REFERENCES workflow_run(workflow_run_id) ON DELETE CASCADE,
    activity_period_id BIGINT REFERENCES xingtu_activity_period(activity_period_id),
    step_name TEXT NOT NULL,
    status TEXT NOT NULL DEFAULT 'running',
    idempotency_key TEXT,
    started_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    finished_at TIMESTAMPTZ,
    summary JSONB NOT NULL DEFAULT '{}'::jsonb,
    error_message TEXT,
    CONSTRAINT chk_workflow_step_name_not_blank CHECK (btrim(step_name) <> ''),
    CONSTRAINT chk_workflow_step_status CHECK (status IN ('running', 'succeeded', 'failed', 'skipped')),
    CONSTRAINT chk_workflow_step_summary_object CHECK (jsonb_typeof(summary) = 'object'),
    UNIQUE (workflow_run_id, activity_period_id, step_name)
);
COMMENT ON TABLE workflow_step IS '工作流阶段台账，记录每个项目各阶段的开始、成功和失败';
CREATE INDEX idx_workflow_step_run ON workflow_step (workflow_run_id, started_at);
CREATE UNIQUE INDEX uq_workflow_step_idempotency_key ON workflow_step (idempotency_key) WHERE idempotency_key IS NOT NULL;

ALTER TABLE xingtu_feishu_source
    ADD COLUMN attempt_count INTEGER NOT NULL DEFAULT 0,
    ADD COLUMN last_attempt_at TIMESTAMPTZ,
    ADD COLUMN next_retry_at TIMESTAMPTZ,
    ADD COLUMN dead_letter_at TIMESTAMPTZ,
    ADD COLUMN ignored_at TIMESTAMPTZ;
ALTER TABLE xingtu_feishu_source
    ADD CONSTRAINT chk_xingtu_feishu_source_attempt_count CHECK (attempt_count >= 0);
COMMENT ON COLUMN xingtu_feishu_source.attempt_count IS '补偿导入累计尝试次数';
COMMENT ON COLUMN xingtu_feishu_source.next_retry_at IS '指数退避后的下次允许重试时间';
COMMENT ON COLUMN xingtu_feishu_source.dead_letter_at IS '达到最大失败次数后进入人工处理队列的时间';
COMMENT ON COLUMN xingtu_feishu_source.ignored_at IS '管理员确认忽略该失败来源的时间';
CREATE INDEX idx_feishu_source_retry_queue
    ON xingtu_feishu_source (next_retry_at, pulled_at, feishu_source_id)
    WHERE is_imported = false AND ignored_at IS NULL AND dead_letter_at IS NULL;

CREATE TABLE xingtu_data_quarantine (
    quarantine_id BIGSERIAL PRIMARY KEY,
    feishu_source_id BIGINT REFERENCES xingtu_feishu_source(feishu_source_id) ON DELETE SET NULL,
    content_config_id BIGINT NOT NULL REFERENCES xingtu_activity_content_config(content_config_id),
    content_type xingtu_content_type NOT NULL,
    unique_key TEXT NOT NULL,
    reason_code TEXT NOT NULL,
    reason_message TEXT NOT NULL,
    raw_fields JSONB NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    resolved_at TIMESTAMPTZ,
    CONSTRAINT chk_xingtu_quarantine_unique_key_not_blank CHECK (btrim(unique_key) <> ''),
    CONSTRAINT chk_xingtu_quarantine_reason_not_blank CHECK (btrim(reason_code) <> ''),
    CONSTRAINT chk_xingtu_quarantine_raw_object CHECK (jsonb_typeof(raw_fields) = 'object')
);
COMMENT ON TABLE xingtu_data_quarantine IS '缺少业务时间等不可安全入库的数据异常隔离区';
CREATE INDEX idx_xingtu_quarantine_unresolved ON xingtu_data_quarantine (created_at DESC) WHERE resolved_at IS NULL;
CREATE UNIQUE INDEX uq_xingtu_quarantine_unresolved_row
    ON xingtu_data_quarantine (
        COALESCE(feishu_source_id, 0), content_config_id, content_type, unique_key, reason_code
    )
    WHERE resolved_at IS NULL;

CREATE TABLE notification_delivery_guard (
    category TEXT NOT NULL,
    scope_key TEXT NOT NULL,
    payload_hash TEXT NOT NULL,
    reserved_until TIMESTAMPTZ NOT NULL,
    last_sent_at TIMESTAMPTZ,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (category, scope_key),
    CONSTRAINT chk_notification_guard_category_not_blank CHECK (btrim(category) <> ''),
    CONSTRAINT chk_notification_guard_scope_not_blank CHECK (btrim(scope_key) <> ''),
    CONSTRAINT chk_notification_guard_hash_not_blank CHECK (btrim(payload_hash) <> '')
);
COMMENT ON TABLE notification_delivery_guard IS '业务通知指纹、发送租约和冷却窗口，避免跨实例重复发送';

CREATE TABLE data_sync_request_nonce (
    nonce TEXT PRIMARY KEY,
    request_timestamp TIMESTAMPTZ NOT NULL,
    expires_at TIMESTAMPTZ NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    CONSTRAINT chk_data_sync_nonce_not_blank CHECK (btrim(nonce) <> '')
);
COMMENT ON TABLE data_sync_request_nonce IS '飞书数据同步签名 nonce，短期保存用于防重放';
CREATE INDEX idx_data_sync_request_nonce_expiry ON data_sync_request_nonce (expires_at);

CREATE TABLE query_cache_revision (
    singleton BOOLEAN PRIMARY KEY DEFAULT true CHECK (singleton),
    revision BIGINT NOT NULL DEFAULT 1,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
INSERT INTO query_cache_revision (singleton) VALUES (true);
COMMENT ON TABLE query_cache_revision IS '跨实例查询缓存版本；业务写入完成后递增';

ALTER TABLE xingtu_login_session
    ADD COLUMN encrypted_payload BYTEA,
    ADD COLUMN encryption_key_id TEXT;
ALTER TABLE xingtu_login_session
    ALTER COLUMN cookie DROP NOT NULL,
    ALTER COLUMN csrf_token DROP NOT NULL;
ALTER TABLE xingtu_login_session
    DROP CONSTRAINT chk_xingtu_login_session_cookie_not_blank,
    DROP CONSTRAINT chk_xingtu_login_session_csrf_not_blank,
    ADD CONSTRAINT chk_xingtu_login_session_protected_payload CHECK (
        encrypted_payload IS NOT NULL
        OR (cookie IS NOT NULL AND btrim(cookie) <> '' AND csrf_token IS NOT NULL AND btrim(csrf_token) <> '')
    );
COMMENT ON COLUMN xingtu_login_session.encrypted_payload IS 'AES-256-GCM 加密后的完整登录态信封';
COMMENT ON COLUMN xingtu_login_session.encryption_key_id IS '登录态加密密钥标识，不包含密钥本身';
COMMENT ON TABLE xingtu_login_session IS '星图登录态表；新写入使用应用层 AES-256-GCM 密文，启动恢复时自动迁移历史明文';

CREATE INDEX idx_feishu_source_config_status_date ON xingtu_feishu_source (content_config_id, import_status, stat_date DESC);
