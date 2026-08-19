DROP INDEX IF EXISTS idx_feishu_source_config_status_date;

DO $$
BEGIN
    IF EXISTS (
        SELECT 1
        FROM xingtu_login_session
        WHERE encrypted_payload IS NOT NULL AND (cookie IS NULL OR csrf_token IS NULL)
    ) THEN
        RAISE EXCEPTION 'cannot roll back encrypted session columns before exporting or deleting encrypted-only login sessions';
    END IF;
END
$$;

ALTER TABLE xingtu_login_session
    DROP CONSTRAINT chk_xingtu_login_session_protected_payload,
    ALTER COLUMN cookie SET NOT NULL,
    ALTER COLUMN csrf_token SET NOT NULL,
    ADD CONSTRAINT chk_xingtu_login_session_cookie_not_blank CHECK (btrim(cookie) <> ''),
    ADD CONSTRAINT chk_xingtu_login_session_csrf_not_blank CHECK (btrim(csrf_token) <> '');
ALTER TABLE xingtu_login_session
    DROP COLUMN encryption_key_id,
    DROP COLUMN encrypted_payload;
COMMENT ON TABLE xingtu_login_session IS '星图登录态表，调试期明文保存 cookie/csrf，后续接企业后端时建议接入密钥系统或加密存储';

DROP TABLE query_cache_revision;
DROP TABLE data_sync_request_nonce;
DROP TABLE notification_delivery_guard;
DROP TABLE xingtu_data_quarantine;

DROP INDEX IF EXISTS idx_feishu_source_retry_queue;
ALTER TABLE xingtu_feishu_source
    DROP CONSTRAINT chk_xingtu_feishu_source_attempt_count,
    DROP COLUMN ignored_at,
    DROP COLUMN dead_letter_at,
    DROP COLUMN next_retry_at,
    DROP COLUMN last_attempt_at,
    DROP COLUMN attempt_count;

DROP TABLE workflow_step;
DROP TABLE workflow_run;
