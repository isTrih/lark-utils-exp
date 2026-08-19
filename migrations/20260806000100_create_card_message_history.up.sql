CREATE TABLE card_message_history (
    card_message_history_id BIGSERIAL PRIMARY KEY,
    message_id TEXT NOT NULL UNIQUE,
    category TEXT NOT NULL,
    summary TEXT NOT NULL,
    receive_id_type xingtu_receive_id_type NOT NULL,
    receive_id TEXT NOT NULL,
    project_name TEXT,
    activity_period_id BIGINT REFERENCES xingtu_activity_period(activity_period_id) ON DELETE SET NULL,
    sent_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    last_recall_attempt_at TIMESTAMPTZ,
    recalled_at TIMESTAMPTZ,
    recall_error TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    CONSTRAINT chk_card_message_history_category
        CHECK (category IN ('error_log', 'audit', 'daily_report')),
    CONSTRAINT chk_card_message_history_message_id_not_blank CHECK (btrim(message_id) <> ''),
    CONSTRAINT chk_card_message_history_summary_not_blank CHECK (btrim(summary) <> ''),
    CONSTRAINT chk_card_message_history_receive_id_not_blank CHECK (btrim(receive_id) <> ''),
    CONSTRAINT chk_card_message_history_project_name_not_blank
        CHECK (project_name IS NULL OR btrim(project_name) <> '')
);

COMMENT ON TABLE card_message_history IS '飞书卡片消息发送历史，用于检索和撤回已发送的错误、审核及日报卡片';
COMMENT ON COLUMN card_message_history.card_message_history_id IS '卡片消息历史内部 ID';
COMMENT ON COLUMN card_message_history.message_id IS '飞书消息 ID，用于撤回消息';
COMMENT ON COLUMN card_message_history.category IS '消息类别：error_log=错误日志，audit=审核，daily_report=日报';
COMMENT ON COLUMN card_message_history.summary IS '不含敏感凭据的消息摘要';
COMMENT ON COLUMN card_message_history.receive_id_type IS '飞书消息接收者 ID 类型';
COMMENT ON COLUMN card_message_history.receive_id IS '飞书消息接收者 ID，群消息通常为 chat_id';
COMMENT ON COLUMN card_message_history.project_name IS '消息关联的项目名称';
COMMENT ON COLUMN card_message_history.activity_period_id IS '消息关联的活动期次内部 ID';
COMMENT ON COLUMN card_message_history.sent_at IS '消息发送成功时间';
COMMENT ON COLUMN card_message_history.last_recall_attempt_at IS '最近一次发起撤回的时间';
COMMENT ON COLUMN card_message_history.recalled_at IS '消息撤回成功时间';
COMMENT ON COLUMN card_message_history.recall_error IS '最近一次撤回失败原因，撤回成功后清空';
COMMENT ON COLUMN card_message_history.created_at IS '记录创建时间';
COMMENT ON COLUMN card_message_history.updated_at IS '记录更新时间';

CREATE INDEX idx_card_message_history_category_sent_at
    ON card_message_history (category, sent_at DESC);
CREATE INDEX idx_card_message_history_sent_at
    ON card_message_history (sent_at DESC);

CREATE TRIGGER trg_card_message_history_updated_at
    BEFORE UPDATE ON card_message_history
    FOR EACH ROW EXECUTE FUNCTION set_updated_at();
