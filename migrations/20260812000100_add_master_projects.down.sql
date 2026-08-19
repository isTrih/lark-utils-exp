DROP TRIGGER IF EXISTS trg_xingtu_project_updated_at ON xingtu_project;

ALTER TABLE xingtu_activity_period
    ADD COLUMN project TEXT,
    ADD COLUMN project_group_id TEXT,
    ADD COLUMN receive_id_type xingtu_receive_id_type NOT NULL DEFAULT 'chat_id',
    ADD COLUMN notice_card_template_id TEXT NOT NULL DEFAULT 'AAqNG59xifQWY',
    ADD COLUMN audit_result_field TEXT NOT NULL DEFAULT '审核结果',
    ADD COLUMN config_snapshot JSONB NOT NULL DEFAULT '{}'::jsonb;

ALTER TABLE xingtu_project_account
    ADD COLUMN project TEXT,
    ADD COLUMN receive_id_type xingtu_receive_id_type NOT NULL DEFAULT 'chat_id',
    ADD COLUMN receive_id TEXT,
    ADD COLUMN login_notice_card_template_id TEXT NOT NULL DEFAULT 'AAqNEWz7H0oIE';

ALTER TABLE xingtu_project_auditor ADD COLUMN project TEXT;

UPDATE xingtu_activity_period period
SET
    project = project.display_name,
    project_group_id = project.notification_receive_id,
    receive_id_type = project.notification_receive_id_type,
    notice_card_template_id = project.audit_notice_card_template_id,
    audit_result_field = project.audit_result_field
FROM xingtu_project project
WHERE project.project_id = period.project_id;

UPDATE xingtu_project_account account
SET
    project = project.project_key,
    receive_id = project.notification_receive_id,
    receive_id_type = project.notification_receive_id_type,
    login_notice_card_template_id = project.login_notice_card_template_id
FROM xingtu_project project
WHERE project.project_id = account.project_id;

UPDATE xingtu_project_auditor auditor
SET project = project.display_name
FROM xingtu_project project
WHERE project.project_id = auditor.project_id;

ALTER TABLE xingtu_activity_period
    ALTER COLUMN project SET NOT NULL,
    ALTER COLUMN project_group_id SET NOT NULL;
ALTER TABLE xingtu_project_account
    ALTER COLUMN project SET NOT NULL,
    ALTER COLUMN receive_id SET NOT NULL;
ALTER TABLE xingtu_project_auditor ALTER COLUMN project SET NOT NULL;

ALTER TABLE xingtu_activity_period
    ADD CONSTRAINT chk_xingtu_activity_period_project_not_blank CHECK (btrim(project) <> ''),
    ADD CONSTRAINT chk_xingtu_activity_period_group_id_not_blank CHECK (btrim(project_group_id) <> ''),
    ADD CONSTRAINT chk_xingtu_activity_period_card_template_not_blank CHECK (btrim(notice_card_template_id) <> ''),
    ADD CONSTRAINT chk_xingtu_activity_period_audit_result_field_not_blank CHECK (btrim(audit_result_field) <> '');
ALTER TABLE xingtu_project_account
    ADD CONSTRAINT chk_xingtu_project_account_project_not_blank CHECK (btrim(project) <> ''),
    ADD CONSTRAINT chk_xingtu_project_account_receive_id_not_blank CHECK (btrim(receive_id) <> ''),
    ADD CONSTRAINT chk_xingtu_project_account_template_not_blank CHECK (btrim(login_notice_card_template_id) <> '');
ALTER TABLE xingtu_project_auditor
    ADD CONSTRAINT chk_xingtu_project_auditor_project_not_blank CHECK (btrim(project) <> '');

COMMENT ON COLUMN xingtu_activity_period.project IS '项目名，例如 ROK';
COMMENT ON COLUMN xingtu_activity_period.project_group_id IS '项目通知群或接收者 ID';
COMMENT ON COLUMN xingtu_activity_period.receive_id_type IS '项目通知接收者 ID 类型，默认 chat_id';
COMMENT ON COLUMN xingtu_activity_period.notice_card_template_id IS '审核通知卡片模板 ID';
COMMENT ON COLUMN xingtu_activity_period.audit_result_field IS '审核表中判断待审核状态的字段名';
COMMENT ON COLUMN xingtu_activity_period.config_snapshot IS '原始配置快照，方便调试和排查配置导入问题';
COMMENT ON COLUMN xingtu_project_account.project IS '项目名，例如 ROK';
COMMENT ON COLUMN xingtu_project_account.receive_id_type IS '登录态失效通知的飞书接收者 ID 类型';
COMMENT ON COLUMN xingtu_project_account.receive_id IS '登录态失效通知接收者，一般是项目群 ID';
COMMENT ON COLUMN xingtu_project_account.login_notice_card_template_id IS '星图登录态失效通知卡片模板 ID';
COMMENT ON COLUMN xingtu_project_auditor.project IS '项目名，例如 ROK';

CREATE TABLE xingtu_activity_auditor (
    activity_auditor_id BIGSERIAL PRIMARY KEY,
    activity_period_id BIGINT NOT NULL REFERENCES xingtu_activity_period(activity_period_id) ON DELETE CASCADE,
    auditor_name TEXT NOT NULL,
    auditor_id TEXT NOT NULL,
    auditor_id_type xingtu_receive_id_type NOT NULL DEFAULT 'user_id',
    sort_order INTEGER NOT NULL DEFAULT 0,
    is_active BOOLEAN NOT NULL DEFAULT true,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (activity_period_id, auditor_id),
    CONSTRAINT chk_xingtu_activity_auditor_name_not_blank CHECK (btrim(auditor_name) <> ''),
    CONSTRAINT chk_xingtu_activity_auditor_id_not_blank CHECK (btrim(auditor_id) <> '')
);

COMMENT ON TABLE xingtu_activity_auditor IS '星图活动审核人配置表，用于生成卡片模板变量 auditor_ids';
COMMENT ON COLUMN xingtu_activity_auditor.activity_auditor_id IS '审核人配置 ID';
COMMENT ON COLUMN xingtu_activity_auditor.activity_period_id IS '所属活动期次 ID';
COMMENT ON COLUMN xingtu_activity_auditor.auditor_name IS '审核人姓名';
COMMENT ON COLUMN xingtu_activity_auditor.auditor_id IS '审核人在飞书中的 ID';
COMMENT ON COLUMN xingtu_activity_auditor.auditor_id_type IS '审核人 ID 类型';
COMMENT ON COLUMN xingtu_activity_auditor.sort_order IS '排序值，用于稳定生成 auditor_ids';
COMMENT ON COLUMN xingtu_activity_auditor.is_active IS '是否启用该审核人';

INSERT INTO xingtu_activity_auditor (
    activity_period_id, auditor_name, auditor_id, auditor_id_type, sort_order, is_active
)
SELECT
    period.activity_period_id,
    auditor.auditor_name,
    auditor.auditor_id,
    auditor.auditor_id_type,
    auditor.sort_order,
    auditor.is_active
FROM xingtu_activity_period period
JOIN xingtu_project_auditor auditor ON auditor.project_id = period.project_id;

CREATE INDEX idx_xingtu_activity_auditor_period_active
    ON xingtu_activity_auditor (activity_period_id, is_active, sort_order);
CREATE TRIGGER trg_xingtu_activity_auditor_updated_at
    BEFORE UPDATE ON xingtu_activity_auditor
    FOR EACH ROW EXECUTE FUNCTION set_updated_at();

ALTER TABLE xingtu_activity_period
    DROP CONSTRAINT fk_xingtu_activity_period_project_account,
    DROP CONSTRAINT uq_xingtu_activity_period_project_period,
    DROP CONSTRAINT fk_xingtu_activity_period_project;
ALTER TABLE xingtu_project_account
    DROP CONSTRAINT uq_xingtu_project_account_project_account,
    DROP CONSTRAINT fk_xingtu_project_account_project;
ALTER TABLE xingtu_project_auditor
    DROP CONSTRAINT uq_xingtu_project_auditor_project_auditor,
    DROP CONSTRAINT fk_xingtu_project_auditor_project;

ALTER TABLE xingtu_activity_period
    ADD CONSTRAINT xingtu_activity_period_project_period_key UNIQUE (project, period),
    ADD CONSTRAINT xingtu_activity_period_xingtu_account_id_fkey
    FOREIGN KEY (xingtu_account_id) REFERENCES xingtu_project_account(xingtu_account_id);
ALTER TABLE xingtu_project_auditor
    ADD CONSTRAINT xingtu_project_auditor_project_auditor_id_key UNIQUE (project, auditor_id);

ALTER TABLE xingtu_activity_period ALTER COLUMN xingtu_account_id DROP NOT NULL;

DROP INDEX IF EXISTS idx_xingtu_project_auditor_project_id_active;
DROP INDEX IF EXISTS idx_xingtu_project_account_project_id;
DROP INDEX IF EXISTS idx_xingtu_activity_period_project_id;
DROP INDEX IF EXISTS uq_xingtu_project_account_one_default;

ALTER TABLE xingtu_project_auditor DROP COLUMN project_id;
ALTER TABLE xingtu_project_account DROP COLUMN is_default, DROP COLUMN project_id;
ALTER TABLE xingtu_activity_period DROP COLUMN project_id;

CREATE INDEX idx_xingtu_project_account_project ON xingtu_project_account (project);
CREATE INDEX idx_xingtu_project_auditor_project_active
    ON xingtu_project_auditor (project, is_active, sort_order);
CREATE INDEX idx_xingtu_activity_period_project_month
    ON xingtu_activity_period (project, task_month);

DROP TABLE xingtu_project;
