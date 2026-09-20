CREATE TABLE xingtu_audit_config (
    audit_config_id BIGSERIAL PRIMARY KEY,
    config_name TEXT NOT NULL UNIQUE,
    feishu_app_id BIGINT REFERENCES xingtu_feishu_app(feishu_app_id) ON DELETE RESTRICT,
    card_template_id TEXT NOT NULL,
    audit_result_field TEXT NOT NULL DEFAULT '审核结果',
    is_active BOOLEAN NOT NULL DEFAULT true,
    remark TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    CONSTRAINT chk_xingtu_audit_config_name_not_blank CHECK (btrim(config_name) <> ''),
    CONSTRAINT chk_xingtu_audit_config_name_trimmed CHECK (config_name = btrim(config_name)),
    CONSTRAINT chk_xingtu_audit_config_template_not_blank CHECK (btrim(card_template_id) <> ''),
    CONSTRAINT chk_xingtu_audit_config_result_field_not_blank CHECK (btrim(audit_result_field) <> '')
);

CREATE TABLE xingtu_audit_notification_target (
    audit_notification_target_id BIGSERIAL PRIMARY KEY,
    audit_config_id BIGINT NOT NULL REFERENCES xingtu_audit_config(audit_config_id) ON DELETE CASCADE,
    target_name TEXT NOT NULL,
    receive_id_type xingtu_receive_id_type NOT NULL DEFAULT 'chat_id',
    receive_id TEXT NOT NULL,
    sort_order INTEGER NOT NULL DEFAULT 0,
    is_active BOOLEAN NOT NULL DEFAULT true,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    CONSTRAINT uq_xingtu_audit_target_config_id UNIQUE (audit_config_id, audit_notification_target_id),
    CONSTRAINT uq_xingtu_audit_target_receiver UNIQUE (audit_config_id, receive_id_type, receive_id),
    CONSTRAINT chk_xingtu_audit_target_name_not_blank CHECK (btrim(target_name) <> ''),
    CONSTRAINT chk_xingtu_audit_target_name_trimmed CHECK (target_name = btrim(target_name)),
    CONSTRAINT chk_xingtu_audit_target_receive_id_not_blank CHECK (btrim(receive_id) <> '')
);

CREATE TABLE xingtu_audit_notification_auditor (
    audit_notification_auditor_id BIGSERIAL PRIMARY KEY,
    audit_notification_target_id BIGINT NOT NULL
        REFERENCES xingtu_audit_notification_target(audit_notification_target_id) ON DELETE CASCADE,
    auditor_name TEXT NOT NULL,
    auditor_id TEXT NOT NULL,
    auditor_id_type xingtu_receive_id_type NOT NULL DEFAULT 'user_id',
    sort_order INTEGER NOT NULL DEFAULT 0,
    is_active BOOLEAN NOT NULL DEFAULT true,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    CONSTRAINT uq_xingtu_audit_target_auditor UNIQUE (audit_notification_target_id, auditor_id),
    CONSTRAINT chk_xingtu_audit_auditor_name_not_blank CHECK (btrim(auditor_name) <> ''),
    CONSTRAINT chk_xingtu_audit_auditor_id_not_blank CHECK (btrim(auditor_id) <> '')
);

CREATE TABLE xingtu_project_audit_config_binding (
    project_id BIGINT PRIMARY KEY REFERENCES xingtu_project(project_id) ON DELETE CASCADE,
    audit_config_id BIGINT NOT NULL REFERENCES xingtu_audit_config(audit_config_id) ON DELETE RESTRICT,
    audit_notification_target_id BIGINT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    CONSTRAINT fk_xingtu_project_audit_binding_target
        FOREIGN KEY (audit_config_id, audit_notification_target_id)
        REFERENCES xingtu_audit_notification_target(audit_config_id, audit_notification_target_id)
        ON DELETE RESTRICT
);

CREATE INDEX idx_xingtu_audit_config_app ON xingtu_audit_config(feishu_app_id);
CREATE INDEX idx_xingtu_audit_target_config
    ON xingtu_audit_notification_target(audit_config_id, is_active, sort_order);
CREATE INDEX idx_xingtu_audit_auditor_target
    ON xingtu_audit_notification_auditor(audit_notification_target_id, is_active, sort_order);
CREATE INDEX idx_xingtu_project_audit_binding_config
    ON xingtu_project_audit_config_binding(audit_config_id, audit_notification_target_id);

CREATE TRIGGER trg_xingtu_audit_config_updated_at
    BEFORE UPDATE ON xingtu_audit_config
    FOR EACH ROW EXECUTE FUNCTION set_updated_at();
CREATE TRIGGER trg_xingtu_audit_target_updated_at
    BEFORE UPDATE ON xingtu_audit_notification_target
    FOR EACH ROW EXECUTE FUNCTION set_updated_at();
CREATE TRIGGER trg_xingtu_audit_auditor_updated_at
    BEFORE UPDATE ON xingtu_audit_notification_auditor
    FOR EACH ROW EXECUTE FUNCTION set_updated_at();
CREATE TRIGGER trg_xingtu_project_audit_binding_updated_at
    BEFORE UPDATE ON xingtu_project_audit_config_binding
    FOR EACH ROW EXECUTE FUNCTION set_updated_at();

COMMENT ON TABLE xingtu_audit_config IS
    '可被多个项目复用的审核配置，统一定义发送应用、卡片模板和审核结果字段';
COMMENT ON COLUMN xingtu_audit_config.feishu_app_id IS
    '审核通知发送应用；历史回填为空时兼容复用项目数据处理应用，新配置必须显式指定';
COMMENT ON TABLE xingtu_audit_notification_target IS
    '审核配置内的通知方案；每项定义一个接收对象及其独立审核员集合';
COMMENT ON TABLE xingtu_audit_notification_auditor IS
    '通知方案内需要在审核卡片中 @ 的审核员';
COMMENT ON TABLE xingtu_project_audit_config_binding IS
    '项目选择一份审核配置及该配置下的一个通知方案';

-- 按完整审核组合去重迁移；应用、模板、群聊或审核员集合任一不同都不会误合并。
CREATE TEMP TABLE _xingtu_legacy_audit_bundle ON COMMIT DROP AS
SELECT project.project_id,
    COALESCE(audit_binding.feishu_app_id, data_binding.feishu_app_id) AS feishu_app_id,
    jsonb_build_object(
        'feishu_app_id', COALESCE(audit_binding.feishu_app_id, data_binding.feishu_app_id),
        'card_template_id', project.audit_notice_card_template_id,
        'audit_result_field', project.audit_result_field,
        'receive_id_type', project.notification_receive_id_type::text,
        'receive_id', project.notification_receive_id,
        'auditors', COALESCE((
            SELECT jsonb_agg(
                jsonb_build_array(
                    auditor.auditor_name, auditor.auditor_id,
                    auditor.auditor_id_type::text, auditor.sort_order, auditor.is_active
                ) ORDER BY auditor.auditor_id, auditor.auditor_name,
                    auditor.auditor_id_type::text, auditor.sort_order, auditor.is_active
            )
            FROM xingtu_project_auditor auditor
            WHERE auditor.project_id = project.project_id
        ), '[]'::jsonb)
    )::text AS signature
FROM xingtu_project project
LEFT JOIN xingtu_project_audit_notice_feishu_app_binding audit_binding
    ON audit_binding.project_id = project.project_id
LEFT JOIN xingtu_project_feishu_app_binding data_binding
    ON data_binding.project_id = project.project_id;

CREATE TEMP TABLE _xingtu_legacy_audit_map ON COMMIT DROP AS
SELECT bundle.project_id,
    min(bundle.project_id) OVER (PARTITION BY bundle.signature) AS representative_project_id
FROM _xingtu_legacy_audit_bundle bundle;

INSERT INTO xingtu_audit_config (
    audit_config_id, config_name, feishu_app_id, card_template_id,
    audit_result_field, is_active, remark
)
SELECT project.project_id,
    project.project_key || ' 审核配置',
    bundle.feishu_app_id,
    project.audit_notice_card_template_id,
    project.audit_result_field,
    true,
    '由相同的项目历史审核通知配置合并迁移'
FROM xingtu_project project
JOIN _xingtu_legacy_audit_bundle bundle ON bundle.project_id = project.project_id
JOIN _xingtu_legacy_audit_map mapping ON mapping.project_id = project.project_id
WHERE mapping.representative_project_id = project.project_id;

INSERT INTO xingtu_audit_notification_target (
    audit_notification_target_id, audit_config_id, target_name,
    receive_id_type, receive_id, sort_order, is_active
)
SELECT project.project_id, project.project_id, project.display_name || ' 默认审核群',
    project.notification_receive_id_type, project.notification_receive_id, 0, true
FROM xingtu_project project
JOIN _xingtu_legacy_audit_map mapping ON mapping.project_id = project.project_id
WHERE mapping.representative_project_id = project.project_id
  AND project.notification_receive_id IS NOT NULL;

INSERT INTO xingtu_audit_notification_auditor (
    audit_notification_auditor_id, audit_notification_target_id,
    auditor_name, auditor_id, auditor_id_type, sort_order, is_active
)
SELECT auditor.project_auditor_id, auditor.project_id,
    auditor.auditor_name, auditor.auditor_id, auditor.auditor_id_type,
    auditor.sort_order, auditor.is_active
FROM xingtu_project_auditor auditor
WHERE EXISTS (
    SELECT 1 FROM xingtu_audit_notification_target target
    WHERE target.audit_notification_target_id = auditor.project_id
);

INSERT INTO xingtu_project_audit_config_binding (
    project_id, audit_config_id, audit_notification_target_id
)
SELECT project.project_id, mapping.representative_project_id, mapping.representative_project_id
FROM xingtu_project project
JOIN _xingtu_legacy_audit_map mapping ON mapping.project_id = project.project_id
WHERE project.notification_receive_id IS NOT NULL;

SELECT setval(
    pg_get_serial_sequence('xingtu_audit_config', 'audit_config_id'),
    GREATEST(COALESCE((SELECT max(audit_config_id) FROM xingtu_audit_config), 0), 1),
    EXISTS (SELECT 1 FROM xingtu_audit_config)
);
SELECT setval(
    pg_get_serial_sequence('xingtu_audit_notification_target', 'audit_notification_target_id'),
    GREATEST(COALESCE((SELECT max(audit_notification_target_id) FROM xingtu_audit_notification_target), 0), 1),
    EXISTS (SELECT 1 FROM xingtu_audit_notification_target)
);
SELECT setval(
    pg_get_serial_sequence('xingtu_audit_notification_auditor', 'audit_notification_auditor_id'),
    GREATEST(COALESCE((SELECT max(audit_notification_auditor_id) FROM xingtu_audit_notification_auditor), 0), 1),
    EXISTS (SELECT 1 FROM xingtu_audit_notification_auditor)
);
