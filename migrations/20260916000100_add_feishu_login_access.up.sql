CREATE TABLE xingtu_feishu_app_project_access (
    feishu_app_id BIGINT NOT NULL
        REFERENCES xingtu_feishu_app(feishu_app_id) ON DELETE CASCADE,
    project_id BIGINT NOT NULL
        REFERENCES xingtu_project(project_id) ON DELETE CASCADE,
    can_view BOOLEAN NOT NULL DEFAULT true,
    can_manage BOOLEAN NOT NULL DEFAULT false,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (feishu_app_id, project_id),
    CONSTRAINT chk_xingtu_feishu_app_project_access_manage
        CHECK (NOT can_manage OR can_view)
);

CREATE INDEX idx_xingtu_feishu_app_project_access_project
    ON xingtu_feishu_app_project_access(project_id, feishu_app_id);

CREATE TRIGGER trg_xingtu_feishu_app_project_access_updated_at
    BEFORE UPDATE ON xingtu_feishu_app_project_access
    FOR EACH ROW EXECUTE FUNCTION set_updated_at();

COMMENT ON TABLE xingtu_feishu_app_project_access IS
    '非默认飞书应用登录后的项目数据可见及期次配置权限；默认应用不受此表限制';

CREATE TABLE auth_session (
    session_id TEXT PRIMARY KEY,
    union_id TEXT NOT NULL,
    open_id TEXT NOT NULL,
    user_name TEXT NOT NULL,
    avatar_url TEXT,
    feishu_app_id BIGINT
        REFERENCES xingtu_feishu_app(feishu_app_id) ON DELETE CASCADE,
    is_default_app BOOLEAN NOT NULL,
    issued_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    expires_at TIMESTAMPTZ NOT NULL,
    revoked_at TIMESTAMPTZ,
    last_seen_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    CONSTRAINT chk_auth_session_identity
        CHECK (btrim(session_id) <> '' AND btrim(union_id) <> '' AND btrim(open_id) <> ''),
    CONSTRAINT chk_auth_session_app_scope
        CHECK ((is_default_app AND feishu_app_id IS NULL)
            OR (NOT is_default_app AND feishu_app_id IS NOT NULL)),
    CONSTRAINT chk_auth_session_expiry CHECK (expires_at > issued_at)
);

CREATE INDEX idx_auth_session_active
    ON auth_session(expires_at, revoked_at)
    WHERE revoked_at IS NULL;

CREATE INDEX idx_auth_session_user
    ON auth_session(union_id, issued_at DESC);

COMMENT ON TABLE auth_session IS
    '飞书 OAuth 登录换取的服务端 JWT 会话；仅保存用户展示信息，不保存飞书 user_access_token';
