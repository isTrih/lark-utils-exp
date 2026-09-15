ALTER TABLE xingtu_activity_content_config
    ALTER COLUMN audit_table_id SET NOT NULL;

COMMENT ON COLUMN xingtu_activity_content_config.audit_table_id IS '审核表 ID';
