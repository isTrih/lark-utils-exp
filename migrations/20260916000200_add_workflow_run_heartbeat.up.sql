ALTER TABLE workflow_run
    ADD COLUMN heartbeat_at TIMESTAMPTZ NOT NULL DEFAULT now();

CREATE INDEX idx_workflow_run_running_heartbeat
    ON workflow_run (heartbeat_at)
    WHERE status = 'running';

COMMENT ON COLUMN workflow_run.heartbeat_at IS
    '运行实例最近一次主动心跳；孤儿恢复只回收超过安全窗口未更新心跳的运行';

CREATE TABLE workflow_execution_lease (
    singleton BOOLEAN PRIMARY KEY DEFAULT true CHECK (singleton),
    workflow_run_id BIGINT REFERENCES workflow_run(workflow_run_id) ON DELETE SET NULL,
    lease_until TIMESTAMPTZ NOT NULL DEFAULT '-infinity',
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

INSERT INTO workflow_execution_lease (singleton) VALUES (true);

COMMENT ON TABLE workflow_execution_lease IS
    '跨实例写工作流短事务租约；避免数据库代理中长期事务锁断连';
