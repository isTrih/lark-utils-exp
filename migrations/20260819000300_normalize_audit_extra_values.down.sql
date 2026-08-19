-- 已规范化的业务值不应重新包装为飞书传输结构；回滚仅移除查询兼容函数。
DROP FUNCTION IF EXISTS normalize_audit_extra_field_value(JSONB);

COMMENT ON COLUMN video_content.audit_extra IS
    '审核扩展字段；从审核表中所有“【额外】”前缀字段同步，去掉前缀后按原 JSON 类型保存';
COMMENT ON COLUMN live_session.audit_extra IS
    '审核扩展字段；从审核表中所有“【额外】”前缀字段同步，去掉前缀后按原 JSON 类型保存';
