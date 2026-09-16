DROP TABLE IF EXISTS auth_session;
DROP TRIGGER IF EXISTS trg_xingtu_feishu_app_project_access_updated_at
    ON xingtu_feishu_app_project_access;
DROP TABLE IF EXISTS xingtu_feishu_app_project_access;
