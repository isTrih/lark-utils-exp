ALTER TABLE live_session
    ADD COLUMN label TEXT;

COMMENT ON COLUMN live_session.label IS '审核标签；从直播审核表的"审核标签"字段随审核结果同步';
