CREATE TABLE xingtu_feishu_app (
    feishu_app_id BIGSERIAL PRIMARY KEY,
    app_id TEXT NOT NULL,
    display_name TEXT NOT NULL,
    encrypted_app_secret BYTEA NOT NULL,
    encryption_key_id TEXT NOT NULL,
    is_active BOOLEAN NOT NULL DEFAULT true,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    CONSTRAINT uq_xingtu_feishu_app_app_id UNIQUE (app_id),
    CONSTRAINT chk_xingtu_feishu_app_app_id
        CHECK (btrim(app_id) <> ''),
    CONSTRAINT chk_xingtu_feishu_app_display_name
        CHECK (btrim(display_name) <> ''),
    CONSTRAINT chk_xingtu_feishu_app_secret
        CHECK (octet_length(encrypted_app_secret) > 29),
    CONSTRAINT chk_xingtu_feishu_app_key_id
        CHECK (btrim(encryption_key_id) <> '')
);

CREATE TABLE xingtu_project_feishu_app_binding (
    project_id BIGINT PRIMARY KEY
        REFERENCES xingtu_project(project_id) ON DELETE CASCADE,
    feishu_app_id BIGINT NOT NULL
        REFERENCES xingtu_feishu_app(feishu_app_id) ON DELETE RESTRICT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX idx_xingtu_project_feishu_app_binding_app
    ON xingtu_project_feishu_app_binding(feishu_app_id);

CREATE TRIGGER trg_xingtu_feishu_app_updated_at
    BEFORE UPDATE ON xingtu_feishu_app
    FOR EACH ROW EXECUTE FUNCTION set_updated_at();

CREATE TRIGGER trg_xingtu_project_feishu_app_binding_updated_at
    BEFORE UPDATE ON xingtu_project_feishu_app_binding
    FOR EACH ROW EXECUTE FUNCTION set_updated_at();

COMMENT ON TABLE xingtu_feishu_app IS
    '可被多个项目复用的飞书开放平台应用；APP_SECRET 仅以 AES-256-GCM 密文保存';
COMMENT ON COLUMN xingtu_feishu_app.encrypted_app_secret IS
    '版本化 AES-256-GCM 密文信封，AAD 绑定 feishu_app_id 与 app_id';
COMMENT ON TABLE xingtu_project_feishu_app_binding IS
    '项目与飞书开放平台应用的多对一绑定';

ALTER TABLE card_message_history
    ADD COLUMN project_id BIGINT REFERENCES xingtu_project(project_id) ON DELETE SET NULL;

UPDATE card_message_history history
SET project_id = period.project_id
FROM xingtu_activity_period period
WHERE history.activity_period_id = period.activity_period_id;

CREATE INDEX idx_card_message_history_project_sent_at
    ON card_message_history(project_id, sent_at DESC);
