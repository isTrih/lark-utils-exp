CREATE TABLE xingtu_project_audit_notice_feishu_app_binding (
    project_id BIGINT PRIMARY KEY
        REFERENCES xingtu_project(project_id) ON DELETE CASCADE,
    feishu_app_id BIGINT NOT NULL
        REFERENCES xingtu_feishu_app(feishu_app_id) ON DELETE RESTRICT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX idx_xingtu_project_audit_notice_feishu_app_binding_app
    ON xingtu_project_audit_notice_feishu_app_binding(feishu_app_id);

CREATE TRIGGER trg_xingtu_project_audit_notice_feishu_app_binding_updated_at
    BEFORE UPDATE ON xingtu_project_audit_notice_feishu_app_binding
    FOR EACH ROW EXECUTE FUNCTION set_updated_at();

COMMENT ON TABLE xingtu_project_audit_notice_feishu_app_binding IS
    '项目审核通知发送应用覆盖；未绑定时复用项目数据处理应用';

ALTER TABLE card_message_history
    ADD COLUMN sender_feishu_app_id BIGINT
        REFERENCES xingtu_feishu_app(feishu_app_id) ON DELETE SET NULL;

COMMENT ON COLUMN card_message_history.sender_feishu_app_id IS
    '实际发送卡片的数据库飞书应用 ID；环境变量默认应用为 NULL';
