DROP TRIGGER IF EXISTS trg_xingtu_project_audit_binding_updated_at
    ON xingtu_project_audit_config_binding;
DROP TRIGGER IF EXISTS trg_xingtu_audit_auditor_updated_at
    ON xingtu_audit_notification_auditor;
DROP TRIGGER IF EXISTS trg_xingtu_audit_target_updated_at
    ON xingtu_audit_notification_target;
DROP TRIGGER IF EXISTS trg_xingtu_audit_config_updated_at ON xingtu_audit_config;

DROP TABLE IF EXISTS xingtu_project_audit_config_binding;
DROP TABLE IF EXISTS xingtu_audit_notification_auditor;
DROP TABLE IF EXISTS xingtu_audit_notification_target;
DROP TABLE IF EXISTS xingtu_audit_config;
