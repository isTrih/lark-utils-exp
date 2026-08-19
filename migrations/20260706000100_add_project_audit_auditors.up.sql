CREATE TABLE xingtu_project_auditor (
    project_auditor_id BIGSERIAL PRIMARY KEY,
    project TEXT NOT NULL,
    auditor_name TEXT NOT NULL,
    auditor_id TEXT NOT NULL,
    auditor_id_type xingtu_receive_id_type NOT NULL DEFAULT 'user_id',
    sort_order INTEGER NOT NULL DEFAULT 0,
    is_active BOOLEAN NOT NULL DEFAULT true,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (project, auditor_id),
    CONSTRAINT chk_xingtu_project_auditor_project_not_blank CHECK (btrim(project) <> ''),
    CONSTRAINT chk_xingtu_project_auditor_name_not_blank CHECK (btrim(auditor_name) <> ''),
    CONSTRAINT chk_xingtu_project_auditor_id_not_blank CHECK (btrim(auditor_id) <> '')
);
COMMENT ON TABLE xingtu_project_auditor IS '星图项目级审核人配置表，用于生成审核通知卡片模板变量 auditor_ids';
COMMENT ON COLUMN xingtu_project_auditor.project_auditor_id IS '项目审核人配置 ID';
COMMENT ON COLUMN xingtu_project_auditor.project IS '项目名，例如 ROK';
COMMENT ON COLUMN xingtu_project_auditor.auditor_name IS '审核人姓名';
COMMENT ON COLUMN xingtu_project_auditor.auditor_id IS '审核人在飞书中的 ID';
COMMENT ON COLUMN xingtu_project_auditor.auditor_id_type IS '审核人 ID 类型';
COMMENT ON COLUMN xingtu_project_auditor.sort_order IS '排序值，用于稳定生成 auditor_ids';
COMMENT ON COLUMN xingtu_project_auditor.is_active IS '是否启用该项目审核人';
COMMENT ON COLUMN xingtu_project_auditor.created_at IS '创建时间';
COMMENT ON COLUMN xingtu_project_auditor.updated_at IS '更新时间';
CREATE INDEX idx_xingtu_project_auditor_project_active
    ON xingtu_project_auditor (project, is_active, sort_order);

INSERT INTO xingtu_project_auditor (
    project,
    auditor_name,
    auditor_id,
    auditor_id_type,
    sort_order,
    is_active
)
SELECT
    p.project,
    (array_agg(a.auditor_name ORDER BY a.sort_order, a.activity_auditor_id))[1],
    a.auditor_id,
    (array_agg(a.auditor_id_type ORDER BY a.sort_order, a.activity_auditor_id))[1],
    MIN(a.sort_order),
    bool_and(a.is_active)
FROM xingtu_activity_auditor a
JOIN xingtu_activity_period p
    ON p.activity_period_id = a.activity_period_id
WHERE p.is_active = true
GROUP BY p.project, a.auditor_id
ON CONFLICT (project, auditor_id) DO NOTHING;

CREATE TRIGGER trg_xingtu_project_auditor_updated_at
    BEFORE UPDATE ON xingtu_project_auditor
    FOR EACH ROW EXECUTE FUNCTION set_updated_at();
