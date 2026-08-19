DROP TRIGGER IF EXISTS trg_live_session_updated_at ON live_session;
DROP TRIGGER IF EXISTS trg_video_content_updated_at ON video_content;
DROP TRIGGER IF EXISTS trg_xingtu_feishu_source_updated_at ON xingtu_feishu_source;
DROP TRIGGER IF EXISTS trg_xingtu_activity_auditor_updated_at ON xingtu_activity_auditor;
DROP TRIGGER IF EXISTS trg_xingtu_activity_content_config_updated_at ON xingtu_activity_content_config;
DROP TRIGGER IF EXISTS trg_xingtu_activity_period_updated_at ON xingtu_activity_period;

DROP FUNCTION IF EXISTS set_updated_at();

DROP TABLE IF EXISTS video_daily_metric_import_history;
DROP TABLE IF EXISTS live_session;
DROP TABLE IF EXISTS video_daily_metric;
DROP TABLE IF EXISTS video_content;
DROP TABLE IF EXISTS xingtu_feishu_source;
DROP TABLE IF EXISTS xingtu_activity_auditor;
DROP TABLE IF EXISTS xingtu_activity_content_config;
DROP TABLE IF EXISTS xingtu_activity_period;

DROP TYPE IF EXISTS xingtu_import_status;
DROP TYPE IF EXISTS xingtu_source_trigger_type;
DROP TYPE IF EXISTS xingtu_spreadsheet_url_update_mode;
DROP TYPE IF EXISTS xingtu_receive_id_type;
DROP TYPE IF EXISTS xingtu_content_type;
