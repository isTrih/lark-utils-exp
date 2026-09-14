DROP TABLE IF EXISTS xingtu_project_feishu_app_binding;
DROP TABLE IF EXISTS xingtu_feishu_app;
ALTER TABLE card_message_history DROP COLUMN IF EXISTS project_id;
