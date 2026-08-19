CREATE INDEX idx_video_content_normalized_label
    ON video_content (lower(NULLIF(btrim(label), '')))
    WHERE NULLIF(btrim(label), '') IS NOT NULL;

CREATE INDEX idx_video_content_config_publish_time
    ON video_content (content_config_id, publish_time);

COMMENT ON INDEX idx_video_content_normalized_label IS
    '支持审核标签忽略大小写精确检索和周报分组';
COMMENT ON INDEX idx_video_content_config_publish_time IS
    '支持按内容配置和北京时间发布时间范围查询视频';
