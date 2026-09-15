UPDATE workflow_run
SET status = 'failed'
WHERE status = 'partial_failed';

ALTER TABLE workflow_run
    DROP CONSTRAINT chk_workflow_run_status;

ALTER TABLE workflow_run
    ADD CONSTRAINT chk_workflow_run_status
    CHECK (status IN ('running', 'succeeded', 'failed', 'blocked'));
