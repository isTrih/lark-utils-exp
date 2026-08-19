ALTER TABLE video_content
    ADD COLUMN label TEXT;

COMMENT ON COLUMN video_content.label IS '审核标签；从视频审核表的“审核标签”字段随审核结果同步';
