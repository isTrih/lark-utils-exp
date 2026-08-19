ALTER TABLE video_content
    ADD COLUMN audit_extra JSONB NOT NULL DEFAULT '{}'::jsonb,
    ADD CONSTRAINT chk_video_content_audit_extra_object
        CHECK (jsonb_typeof(audit_extra) = 'object');

ALTER TABLE live_session
    ADD COLUMN audit_extra JSONB NOT NULL DEFAULT '{}'::jsonb,
    ADD CONSTRAINT chk_live_session_audit_extra_object
        CHECK (jsonb_typeof(audit_extra) = 'object');

COMMENT ON COLUMN video_content.audit_extra IS
    '审核扩展字段；从审核表中所有“【额外】”前缀字段同步，去掉前缀后按原 JSON 类型保存';
COMMENT ON COLUMN live_session.audit_extra IS
    '审核扩展字段；从审核表中所有“【额外】”前缀字段同步，去掉前缀后按原 JSON 类型保存';
