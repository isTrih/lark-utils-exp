CREATE TYPE xingtu_session_status AS ENUM ('unknown', 'valid', 'invalid');
COMMENT ON TYPE xingtu_session_status IS '星图登录态状态：unknown=未检查，valid=有效，invalid=失效';

ALTER TYPE xingtu_source_trigger_type ADD VALUE IF NOT EXISTS 'night';

CREATE TABLE xingtu_project_account (
    xingtu_account_id TEXT PRIMARY KEY,
    project TEXT NOT NULL,
    display_name TEXT,
    receive_id_type xingtu_receive_id_type NOT NULL DEFAULT 'chat_id',
    receive_id TEXT NOT NULL,
    ops_ids TEXT[] NOT NULL DEFAULT '{}'::text[],
    login_notice_card_template_id TEXT NOT NULL DEFAULT 'AAqNEWz7H0oIE',
    login_check_enabled BOOLEAN NOT NULL DEFAULT true,
    session_status xingtu_session_status NOT NULL DEFAULT 'unknown',
    last_checked_at TIMESTAMPTZ,
    last_valid_at TIMESTAMPTZ,
    last_invalid_at TIMESTAMPTZ,
    last_error TEXT,
    is_active BOOLEAN NOT NULL DEFAULT true,
    remark TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    CONSTRAINT chk_xingtu_project_account_id_not_blank CHECK (btrim(xingtu_account_id) <> ''),
    CONSTRAINT chk_xingtu_project_account_project_not_blank CHECK (btrim(project) <> ''),
    CONSTRAINT chk_xingtu_project_account_receive_id_not_blank CHECK (btrim(receive_id) <> ''),
    CONSTRAINT chk_xingtu_project_account_template_not_blank CHECK (btrim(login_notice_card_template_id) <> '')
);
COMMENT ON TABLE xingtu_project_account IS '项目星图账号配置表，一个项目通常对应一个星图账号，用于选择登录态和发送运维通知';
COMMENT ON COLUMN xingtu_project_account.xingtu_account_id IS '项目星图账号稳定 ID，由业务配置传入';
COMMENT ON COLUMN xingtu_project_account.project IS '项目名，例如 ROK';
COMMENT ON COLUMN xingtu_project_account.display_name IS '星图账号展示名';
COMMENT ON COLUMN xingtu_project_account.receive_id_type IS '登录态失效通知的飞书接收者 ID 类型';
COMMENT ON COLUMN xingtu_project_account.receive_id IS '登录态失效通知接收者，一般是项目群 ID';
COMMENT ON COLUMN xingtu_project_account.ops_ids IS '登录态失效时需要 @ 的运维/项目同学 ID 列表';
COMMENT ON COLUMN xingtu_project_account.login_notice_card_template_id IS '星图登录态失效通知卡片模板 ID';
COMMENT ON COLUMN xingtu_project_account.login_check_enabled IS '是否启用该账号的登录态巡检';
COMMENT ON COLUMN xingtu_project_account.session_status IS '最近一次登录态检查状态';
COMMENT ON COLUMN xingtu_project_account.last_checked_at IS '最近一次检查时间';
COMMENT ON COLUMN xingtu_project_account.last_valid_at IS '最近一次检查有效的时间';
COMMENT ON COLUMN xingtu_project_account.last_invalid_at IS '最近一次检查失效的时间';
COMMENT ON COLUMN xingtu_project_account.last_error IS '最近一次检查错误信息';
COMMENT ON COLUMN xingtu_project_account.is_active IS '账号配置是否启用';
COMMENT ON COLUMN xingtu_project_account.remark IS '备注';
COMMENT ON COLUMN xingtu_project_account.created_at IS '创建时间';
COMMENT ON COLUMN xingtu_project_account.updated_at IS '更新时间';
CREATE INDEX idx_xingtu_project_account_project ON xingtu_project_account (project);
CREATE INDEX idx_xingtu_project_account_check ON xingtu_project_account (is_active, login_check_enabled, session_status);

CREATE TABLE xingtu_login_session (
    xingtu_account_id TEXT PRIMARY KEY REFERENCES xingtu_project_account(xingtu_account_id) ON DELETE CASCADE,
    cookie TEXT NOT NULL,
    csrf_token TEXT NOT NULL,
    session_key TEXT,
    user_agent TEXT,
    extra_headers JSONB NOT NULL DEFAULT '{}'::jsonb,
    received_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    CONSTRAINT chk_xingtu_login_session_cookie_not_blank CHECK (btrim(cookie) <> ''),
    CONSTRAINT chk_xingtu_login_session_csrf_not_blank CHECK (btrim(csrf_token) <> ''),
    CONSTRAINT chk_xingtu_login_session_extra_headers_object CHECK (jsonb_typeof(extra_headers) = 'object')
);
COMMENT ON TABLE xingtu_login_session IS '星图登录态表，调试期明文保存 cookie/csrf，后续接企业后端时建议接入密钥系统或加密存储';
COMMENT ON COLUMN xingtu_login_session.xingtu_account_id IS '项目星图账号稳定 ID';
COMMENT ON COLUMN xingtu_login_session.cookie IS '星图 Cookie';
COMMENT ON COLUMN xingtu_login_session.csrf_token IS '星图 CSRF Token';
COMMENT ON COLUMN xingtu_login_session.session_key IS '星图 session_key 请求头';
COMMENT ON COLUMN xingtu_login_session.user_agent IS '浏览器 User-Agent';
COMMENT ON COLUMN xingtu_login_session.extra_headers IS '额外请求头';
COMMENT ON COLUMN xingtu_login_session.received_at IS '服务端接收登录态时间';
COMMENT ON COLUMN xingtu_login_session.updated_at IS '更新时间';

ALTER TABLE xingtu_activity_period
    ADD COLUMN xingtu_account_id TEXT REFERENCES xingtu_project_account(xingtu_account_id);
COMMENT ON COLUMN xingtu_activity_period.xingtu_account_id IS '本期活动使用的项目星图账号 ID，用于选择星图登录态';
CREATE INDEX idx_xingtu_activity_period_xingtu_account ON xingtu_activity_period (xingtu_account_id);

ALTER TABLE xingtu_feishu_source
    ADD COLUMN pull_started_at TIMESTAMPTZ;
COMMENT ON COLUMN xingtu_feishu_source.pull_started_at IS '开始向星图发起拉取的时间；数据归属日期 stat_date 以该时间为准';

COMMENT ON COLUMN xingtu_activity_content_config.trace_enabled IS '是否写入追踪明细；不影响星图链接拉取和审核表同步。当前只有视频会写追踪明细，直播只更新最新数据';

CREATE TRIGGER trg_xingtu_project_account_updated_at BEFORE UPDATE ON xingtu_project_account FOR EACH ROW EXECUTE FUNCTION set_updated_at();
CREATE TRIGGER trg_xingtu_login_session_updated_at BEFORE UPDATE ON xingtu_login_session FOR EACH ROW EXECUTE FUNCTION set_updated_at();
