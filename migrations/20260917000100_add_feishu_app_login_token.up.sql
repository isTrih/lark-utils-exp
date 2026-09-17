CREATE TABLE xingtu_feishu_app_login_token (
    feishu_app_id BIGINT PRIMARY KEY
        REFERENCES xingtu_feishu_app(feishu_app_id) ON DELETE CASCADE,
    token_hash BYTEA NOT NULL,
    token_prefix TEXT NOT NULL,
    remark TEXT,
    is_active BOOLEAN NOT NULL DEFAULT true,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    last_used_at TIMESTAMPTZ,
    CONSTRAINT uq_xingtu_feishu_app_login_token_hash UNIQUE (token_hash),
    CONSTRAINT chk_xingtu_feishu_app_login_token_hash
        CHECK (octet_length(token_hash) = 32),
    CONSTRAINT chk_xingtu_feishu_app_login_token_prefix
        CHECK (token_prefix LIKE 'sk-%')
);

CREATE TRIGGER trg_xingtu_feishu_app_login_token_updated_at
    BEFORE UPDATE ON xingtu_feishu_app_login_token
    FOR EACH ROW EXECUTE FUNCTION set_updated_at();

COMMENT ON TABLE xingtu_feishu_app_login_token IS
    '非默认飞书应用的长期共享登录 Token；仅保存 SHA-256 摘要，轮换或撤销前不过期';
COMMENT ON COLUMN xingtu_feishu_app_login_token.token_prefix IS
    '仅用于管理员识别凭证的脱敏前缀，不可用于登录';
COMMENT ON COLUMN xingtu_feishu_app_login_token.remark IS
    '管理员为应用 Token 设置的用途或分发对象备注';

COMMENT ON TABLE auth_session IS
    '飞书 OAuth 或非默认应用 Token 登录换取的七天 JWT 会话；不保存飞书 user_access_token 或应用 Token 明文';
