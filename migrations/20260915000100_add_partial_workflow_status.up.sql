ALTER TABLE workflow_run
    DROP CONSTRAINT chk_workflow_run_status;

ALTER TABLE workflow_run
    ADD CONSTRAINT chk_workflow_run_status
    CHECK (status IN ('running', 'succeeded', 'partial_failed', 'failed', 'blocked'));

COMMENT ON COLUMN workflow_run.status IS
    '运行状态；partial_failed 表示部分项目失败，但其他项目已继续执行';
