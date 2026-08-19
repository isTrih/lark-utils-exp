ALTER TABLE live_session
    DROP COLUMN IF EXISTS audit_result;

ALTER TABLE live_session
    ALTER COLUMN feishu_source_id SET NOT NULL;

ALTER TABLE video_content
    DROP COLUMN IF EXISTS audit_result;
