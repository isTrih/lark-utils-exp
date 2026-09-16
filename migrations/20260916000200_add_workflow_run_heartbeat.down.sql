DROP TABLE IF EXISTS workflow_execution_lease;

DROP INDEX IF EXISTS idx_workflow_run_running_heartbeat;

ALTER TABLE workflow_run
    DROP COLUMN IF EXISTS heartbeat_at;
