ALTER TABLE video_content
    ADD COLUMN IF NOT EXISTS audit_result TEXT;

ALTER TABLE live_session
    ADD COLUMN IF NOT EXISTS audit_result TEXT;

ALTER TABLE live_session
    ALTER COLUMN feishu_source_id DROP NOT NULL;

COMMENT ON COLUMN video_content.audit_result IS '审核结果；手动登记数据默认写入审核通过，星图数据为空等待审核';
COMMENT ON COLUMN live_session.audit_result IS '审核结果；手动登记数据默认写入审核通过，星图数据为空等待审核';
