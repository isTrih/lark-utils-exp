ALTER TABLE video_content
    DROP COLUMN IF EXISTS audit_extra;

ALTER TABLE live_session
    DROP COLUMN IF EXISTS audit_extra;
