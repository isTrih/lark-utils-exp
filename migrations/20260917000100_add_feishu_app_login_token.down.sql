DROP TABLE IF EXISTS xingtu_feishu_app_login_token;

COMMENT ON TABLE auth_session IS
    '飞书 OAuth 登录换取的服务端 JWT 会话；仅保存用户展示信息，不保存飞书 user_access_token';
