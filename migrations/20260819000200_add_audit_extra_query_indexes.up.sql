CREATE INDEX idx_video_content_audit_extra_gin
    ON video_content USING GIN (audit_extra jsonb_path_ops);

CREATE INDEX idx_live_session_audit_extra_gin
    ON live_session USING GIN (audit_extra jsonb_path_ops);

COMMENT ON INDEX idx_video_content_audit_extra_gin IS
    '加速视频 audit_extra JSONB 包含查询；精确值校验由查询条件二次确认';
COMMENT ON INDEX idx_live_session_audit_extra_gin IS
    '加速直播 audit_extra JSONB 包含查询；精确值校验由查询条件二次确认';
