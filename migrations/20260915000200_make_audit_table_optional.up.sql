ALTER TABLE xingtu_activity_content_config
    ALTER COLUMN audit_table_id DROP NOT NULL;

COMMENT ON COLUMN xingtu_activity_content_config.audit_table_id IS
    '可选审核表 ID；为空时仅同步业务主表，跳过审核表写入、审核结果回写和审核通知统计';
