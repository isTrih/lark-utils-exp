CREATE TEMP TABLE legacy_project_mapping (
    legacy_project TEXT NOT NULL,
    project_key TEXT NOT NULL
) ON COMMIT DROP;

INSERT INTO legacy_project_mapping (legacy_project, project_key)
SELECT DISTINCT btrim(project), btrim(project)
FROM xingtu_project_account
UNION
SELECT DISTINCT btrim(period.project), btrim(account.project)
FROM xingtu_activity_period period
JOIN xingtu_project_account account
    ON account.xingtu_account_id = period.xingtu_account_id
UNION
SELECT DISTINCT btrim(period.project), btrim(period.project)
FROM xingtu_activity_period period
WHERE period.xingtu_account_id IS NULL
  AND EXISTS (
      SELECT 1 FROM xingtu_project_account account
      WHERE btrim(account.project) = btrim(period.project)
  );

DO $$
DECLARE conflicts TEXT;
BEGIN
    SELECT string_agg(format('%s => %s', legacy_project, project_keys), '; ' ORDER BY legacy_project)
    INTO conflicts
    FROM (
        SELECT legacy_project,
            string_agg(DISTINCT quote_literal(project_key), ', ' ORDER BY quote_literal(project_key)) AS project_keys
        FROM legacy_project_mapping
        GROUP BY legacy_project
        HAVING count(DISTINCT project_key) > 1
    ) ambiguous;
    IF conflicts IS NOT NULL THEN
        RAISE EXCEPTION '同一期次项目名通过已绑定账号映射到多个主项目：%', conflicts;
    END IF;

    SELECT string_agg(format('%s => %s', normalized, variants), '; ' ORDER BY normalized)
    INTO conflicts
    FROM (
        SELECT lower(btrim(project)) AS normalized,
            string_agg(DISTINCT quote_literal(btrim(project)), ', ' ORDER BY quote_literal(btrim(project))) AS variants
        FROM xingtu_project_account
        GROUP BY lower(btrim(project))
        HAVING count(DISTINCT btrim(project)) > 1
    ) duplicated;
    IF conflicts IS NOT NULL THEN
        RAISE EXCEPTION '主项目业务键仅大小写不同：%', conflicts;
    END IF;

    SELECT string_agg(format('%s => %s', project_key, display_names), '; ' ORDER BY project_key)
    INTO conflicts
    FROM (
        SELECT mapping.project_key,
            string_agg(DISTINCT quote_literal(btrim(period.project)), ', ' ORDER BY quote_literal(btrim(period.project))) AS display_names
        FROM xingtu_activity_period period
        JOIN legacy_project_mapping mapping ON mapping.legacy_project = btrim(period.project)
        GROUP BY mapping.project_key
        HAVING count(DISTINCT btrim(period.project)) > 1
    ) ambiguous;
    IF conflicts IS NOT NULL THEN
        RAISE EXCEPTION '同一主项目存在多个历史展示名，请先明确 display_name：%', conflicts;
    END IF;

    SELECT string_agg(format('%s/%s', project_key, period), ', ' ORDER BY project_key, period)
    INTO conflicts
    FROM (
        SELECT mapping.project_key, period.period
        FROM xingtu_activity_period period
        JOIN legacy_project_mapping mapping ON mapping.legacy_project = btrim(period.project)
        GROUP BY mapping.project_key, period.period
        HAVING count(*) > 1
    ) duplicated;
    IF conflicts IS NOT NULL THEN
        RAISE EXCEPTION '归一化项目关系后出现重复期次：%', conflicts;
    END IF;

    SELECT string_agg(format('%s/%s', btrim(period.project), period.period), ', ' ORDER BY btrim(period.project), period.period)
    INTO conflicts
    FROM xingtu_activity_period period
    WHERE NOT EXISTS (
        SELECT 1 FROM legacy_project_mapping mapping
        WHERE mapping.legacy_project = btrim(period.project)
    );
    IF conflicts IS NOT NULL THEN
        RAISE EXCEPTION '期次未绑定账号且无法按旧项目名找到账号：%', conflicts;
    END IF;

    SELECT string_agg(format('%s/%s -> %s', btrim(period.project), period.period, period.xingtu_account_id), ', ' ORDER BY btrim(period.project), period.period)
    INTO conflicts
    FROM xingtu_activity_period period
    JOIN xingtu_project_account account ON account.xingtu_account_id = period.xingtu_account_id
    WHERE period.is_active = true AND account.is_active = false;
    IF conflicts IS NOT NULL THEN
        RAISE EXCEPTION '启用期次绑定了停用账号：%', conflicts;
    END IF;

    SELECT string_agg(project_key, ', ' ORDER BY project_key)
    INTO conflicts
    FROM (
        SELECT mapping.project_key
        FROM xingtu_activity_period period
        JOIN legacy_project_mapping mapping ON mapping.legacy_project = btrim(period.project)
        GROUP BY mapping.project_key
        HAVING count(DISTINCT (project_group_id, receive_id_type, notice_card_template_id, audit_result_field)) > 1
    ) inconsistent;
    IF conflicts IS NOT NULL THEN
        RAISE EXCEPTION '同一主项目的历史期次通知配置不一致：%', conflicts;
    END IF;

    SELECT string_agg(project, ', ' ORDER BY project)
    INTO conflicts
    FROM (
        SELECT btrim(project) AS project
        FROM xingtu_project_account
        GROUP BY btrim(project)
        HAVING count(DISTINCT (receive_id, receive_id_type, login_notice_card_template_id)) > 1
    ) inconsistent;
    IF conflicts IS NOT NULL THEN
        RAISE EXCEPTION '同一主项目的历史账号通知配置不一致：%', conflicts;
    END IF;

    SELECT string_agg(DISTINCT mapping.project_key, ', ' ORDER BY mapping.project_key)
    INTO conflicts
    FROM xingtu_activity_period period
    JOIN legacy_project_mapping mapping ON mapping.legacy_project = btrim(period.project)
    JOIN xingtu_project_account account ON btrim(account.project) = mapping.project_key
    WHERE (period.receive_id_type, period.project_group_id)
        IS DISTINCT FROM (account.receive_id_type, account.receive_id);
    IF conflicts IS NOT NULL THEN
        RAISE EXCEPTION '项目的期次通知目标与账号通知目标不一致，请先明确统一目标：%', conflicts;
    END IF;

    SELECT string_agg(quote_literal(btrim(auditor.project)), ', ' ORDER BY quote_literal(btrim(auditor.project)))
    INTO conflicts
    FROM xingtu_project_auditor auditor
    WHERE NOT EXISTS (
        SELECT 1 FROM legacy_project_mapping mapping
        WHERE mapping.legacy_project = btrim(auditor.project)
    );
    IF conflicts IS NOT NULL THEN
        RAISE EXCEPTION '审核员项目无法映射到任何账号或期次：%', conflicts;
    END IF;
END $$;

DELETE FROM legacy_project_mapping first
USING legacy_project_mapping duplicate
WHERE first.ctid < duplicate.ctid
  AND first.legacy_project = duplicate.legacy_project
  AND first.project_key = duplicate.project_key;

ALTER TABLE legacy_project_mapping ADD PRIMARY KEY (legacy_project);

CREATE TABLE xingtu_project (
    project_id BIGSERIAL PRIMARY KEY,
    project_key TEXT NOT NULL UNIQUE,
    display_name TEXT NOT NULL,
    notification_receive_id_type xingtu_receive_id_type NOT NULL DEFAULT 'chat_id',
    notification_receive_id TEXT,
    audit_notice_card_template_id TEXT NOT NULL DEFAULT 'AAqNG59xifQWY',
    audit_result_field TEXT NOT NULL DEFAULT '审核结果',
    login_notice_card_template_id TEXT NOT NULL DEFAULT 'AAqNEWz7H0oIE',
    is_active BOOLEAN NOT NULL DEFAULT true,
    remark TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    CONSTRAINT chk_xingtu_project_key_not_blank CHECK (btrim(project_key) <> ''),
    CONSTRAINT chk_xingtu_project_key_trimmed CHECK (project_key = btrim(project_key)),
    CONSTRAINT chk_xingtu_project_display_name_not_blank CHECK (btrim(display_name) <> ''),
    CONSTRAINT chk_xingtu_project_display_name_trimmed CHECK (display_name = btrim(display_name)),
    CONSTRAINT chk_xingtu_project_notification_receive_id_not_blank CHECK (
        notification_receive_id IS NULL OR btrim(notification_receive_id) <> ''
    ),
    CONSTRAINT chk_xingtu_project_audit_notice_template_not_blank CHECK (
        btrim(audit_notice_card_template_id) <> ''
    ),
    CONSTRAINT chk_xingtu_project_audit_result_field_not_blank CHECK (
        btrim(audit_result_field) <> ''
    ),
    CONSTRAINT chk_xingtu_project_login_notice_template_not_blank CHECK (
        btrim(login_notice_card_template_id) <> ''
    )
);

CREATE UNIQUE INDEX uq_xingtu_project_key_ci ON xingtu_project (lower(project_key));

COMMENT ON TABLE xingtu_project IS '星图主项目配置；统一承载项目身份和通知配置';
COMMENT ON COLUMN xingtu_project.project_id IS '主项目内部稳定 ID';
COMMENT ON COLUMN xingtu_project.project_key IS '主项目稳定业务键，例如 ROK';
COMMENT ON COLUMN xingtu_project.display_name IS '主项目展示名，例如 [ROK]生态';
COMMENT ON COLUMN xingtu_project.notification_receive_id_type IS '项目通知接收者 ID 类型';
COMMENT ON COLUMN xingtu_project.notification_receive_id IS '项目通知群或其他接收者 ID';
COMMENT ON COLUMN xingtu_project.audit_notice_card_template_id IS '项目审核通知卡片模板 ID';
COMMENT ON COLUMN xingtu_project.audit_result_field IS '项目审核表中的审核结果字段名';
COMMENT ON COLUMN xingtu_project.login_notice_card_template_id IS '项目星图登录态/工作流错误通知卡片模板 ID';
COMMENT ON COLUMN xingtu_project.is_active IS '主项目是否启用';
COMMENT ON COLUMN xingtu_project.remark IS '主项目备注';

INSERT INTO xingtu_project (project_key, display_name)
SELECT
    mapping.project_key,
    COALESCE(
        (
            SELECT btrim(period.project)
            FROM xingtu_activity_period period
            JOIN legacy_project_mapping period_mapping
                ON period_mapping.legacy_project = btrim(period.project)
            WHERE period_mapping.project_key = mapping.project_key
            ORDER BY period.is_active DESC, period.task_month DESC, period.activity_period_id DESC
            LIMIT 1
        ),
        mapping.project_key
    )
FROM (SELECT DISTINCT project_key FROM legacy_project_mapping) mapping
WHERE mapping.project_key <> ''
ON CONFLICT (project_key) DO NOTHING;

ALTER TABLE xingtu_activity_period ADD COLUMN project_id BIGINT;
ALTER TABLE xingtu_project_account ADD COLUMN project_id BIGINT;
ALTER TABLE xingtu_project_account ADD COLUMN is_default BOOLEAN NOT NULL DEFAULT false;
ALTER TABLE xingtu_project_auditor ADD COLUMN project_id BIGINT;

UPDATE xingtu_activity_period period
SET project_id = project.project_id
FROM legacy_project_mapping mapping
JOIN xingtu_project project ON project.project_key = mapping.project_key
WHERE mapping.legacy_project = btrim(period.project);

UPDATE xingtu_project_account account
SET project_id = project.project_id
FROM xingtu_project project
WHERE project.project_key = btrim(account.project);

UPDATE xingtu_project_auditor auditor
SET project_id = project.project_id
FROM legacy_project_mapping mapping
JOIN xingtu_project project ON project.project_key = mapping.project_key
WHERE mapping.legacy_project = btrim(auditor.project);

WITH latest_period AS (
    SELECT DISTINCT ON (period.project_id)
        period.project_id,
        period.project_group_id,
        period.receive_id_type,
        period.notice_card_template_id,
        period.audit_result_field
    FROM xingtu_activity_period period
    ORDER BY period.project_id, period.is_active DESC, period.task_month DESC, period.activity_period_id DESC
)
UPDATE xingtu_project project
SET
    notification_receive_id = latest_period.project_group_id,
    notification_receive_id_type = latest_period.receive_id_type,
    audit_notice_card_template_id = latest_period.notice_card_template_id,
    audit_result_field = latest_period.audit_result_field
FROM latest_period
WHERE latest_period.project_id = project.project_id;

WITH referenced AS (
    SELECT DISTINCT ON (period.project_id)
        period.project_id,
        period.xingtu_account_id
    FROM xingtu_activity_period period
    JOIN xingtu_project_account account
        ON account.project_id = period.project_id
        AND account.xingtu_account_id = period.xingtu_account_id
    WHERE period.xingtu_account_id IS NOT NULL AND account.is_active = true
    ORDER BY period.project_id, period.is_active DESC, period.task_month DESC, period.activity_period_id DESC
)
UPDATE xingtu_project_account account
SET is_default = true
FROM referenced
WHERE account.project_id = referenced.project_id
  AND account.xingtu_account_id = referenced.xingtu_account_id;

WITH fallback AS (
    SELECT DISTINCT ON (account.project_id)
        account.project_id,
        account.xingtu_account_id
    FROM xingtu_project_account account
    WHERE account.is_active = true
      AND
      NOT EXISTS (
        SELECT 1
        FROM xingtu_project_account selected
        WHERE selected.project_id = account.project_id AND selected.is_default = true
    )
    ORDER BY account.project_id, account.is_active DESC, account.created_at, account.xingtu_account_id
)
UPDATE xingtu_project_account account
SET is_default = true
FROM fallback
WHERE account.project_id = fallback.project_id
  AND account.xingtu_account_id = fallback.xingtu_account_id;

WITH preferred_account AS (
    SELECT DISTINCT ON (account.project_id)
        account.project_id,
        account.receive_id,
        account.receive_id_type,
        account.login_notice_card_template_id
    FROM xingtu_project_account account
    ORDER BY account.project_id, account.is_default DESC, account.is_active DESC, account.created_at, account.xingtu_account_id
)
UPDATE xingtu_project project
SET
    notification_receive_id = COALESCE(project.notification_receive_id, preferred_account.receive_id),
    notification_receive_id_type = CASE
        WHEN project.notification_receive_id IS NULL THEN preferred_account.receive_id_type
        ELSE project.notification_receive_id_type
    END,
    login_notice_card_template_id = preferred_account.login_notice_card_template_id
FROM preferred_account
WHERE preferred_account.project_id = project.project_id;

UPDATE xingtu_activity_period period
SET xingtu_account_id = account.xingtu_account_id
FROM xingtu_project_account account
WHERE period.project_id = account.project_id
  AND period.xingtu_account_id IS NULL
  AND account.is_default = true;

ALTER TABLE xingtu_project ALTER COLUMN notification_receive_id SET NOT NULL;

ALTER TABLE xingtu_activity_period ALTER COLUMN project_id SET NOT NULL;
ALTER TABLE xingtu_activity_period ALTER COLUMN xingtu_account_id SET NOT NULL;
ALTER TABLE xingtu_project_account ALTER COLUMN project_id SET NOT NULL;
ALTER TABLE xingtu_project_auditor ALTER COLUMN project_id SET NOT NULL;

ALTER TABLE xingtu_activity_period
    DROP CONSTRAINT xingtu_activity_period_project_period_key,
    DROP CONSTRAINT IF EXISTS xingtu_activity_period_xingtu_account_id_fkey;
ALTER TABLE xingtu_project_auditor
    DROP CONSTRAINT xingtu_project_auditor_project_auditor_id_key;

ALTER TABLE xingtu_activity_period
    ADD CONSTRAINT fk_xingtu_activity_period_project
    FOREIGN KEY (project_id) REFERENCES xingtu_project(project_id) ON DELETE RESTRICT,
    ADD CONSTRAINT uq_xingtu_activity_period_project_period UNIQUE (project_id, period);

ALTER TABLE xingtu_project_account
    ADD CONSTRAINT fk_xingtu_project_account_project
    FOREIGN KEY (project_id) REFERENCES xingtu_project(project_id) ON DELETE RESTRICT,
    ADD CONSTRAINT uq_xingtu_project_account_project_account UNIQUE (project_id, xingtu_account_id);

ALTER TABLE xingtu_project_account
    ADD CONSTRAINT chk_xingtu_project_account_default_active
    CHECK (NOT is_default OR is_active);

ALTER TABLE xingtu_project_auditor
    ADD CONSTRAINT fk_xingtu_project_auditor_project
    FOREIGN KEY (project_id) REFERENCES xingtu_project(project_id) ON DELETE RESTRICT,
    ADD CONSTRAINT uq_xingtu_project_auditor_project_auditor UNIQUE (project_id, auditor_id);

ALTER TABLE xingtu_activity_period
    ADD CONSTRAINT fk_xingtu_activity_period_project_account
    FOREIGN KEY (project_id, xingtu_account_id)
    REFERENCES xingtu_project_account(project_id, xingtu_account_id)
    ON UPDATE CASCADE ON DELETE RESTRICT;

DROP TABLE xingtu_activity_auditor;

ALTER TABLE xingtu_activity_period
    DROP COLUMN project,
    DROP COLUMN project_group_id,
    DROP COLUMN receive_id_type,
    DROP COLUMN notice_card_template_id,
    DROP COLUMN audit_result_field,
    DROP COLUMN config_snapshot;

ALTER TABLE xingtu_project_account
    DROP COLUMN project,
    DROP COLUMN receive_id_type,
    DROP COLUMN receive_id,
    DROP COLUMN login_notice_card_template_id;

ALTER TABLE xingtu_project_auditor DROP COLUMN project;

CREATE UNIQUE INDEX uq_xingtu_project_account_one_default
    ON xingtu_project_account (project_id)
    WHERE is_default = true;
CREATE INDEX idx_xingtu_activity_period_project_id
    ON xingtu_activity_period (project_id, task_month DESC);
CREATE INDEX idx_xingtu_project_account_project_id
    ON xingtu_project_account (project_id, is_active);
CREATE INDEX idx_xingtu_project_auditor_project_id_active
    ON xingtu_project_auditor (project_id, is_active, sort_order);

COMMENT ON COLUMN xingtu_activity_period.project_id IS '所属主项目 ID';
COMMENT ON COLUMN xingtu_project_account.project_id IS '所属主项目 ID';
COMMENT ON COLUMN xingtu_project_account.is_default IS '是否为主项目默认星图账号';
COMMENT ON COLUMN xingtu_project_auditor.project_id IS '所属主项目 ID';

CREATE TRIGGER trg_xingtu_project_updated_at
    BEFORE UPDATE ON xingtu_project
    FOR EACH ROW EXECUTE FUNCTION set_updated_at();
