ALTER TABLE xingtu_project_account
    ADD COLUMN daily_video_table_id TEXT;

UPDATE xingtu_project_account
SET daily_video_table_id = 'tbluWMXXIzO4CtSm'
WHERE project = 'ROK'
  AND daily_video_table_id IS NULL;

ALTER TABLE xingtu_project_account
    ADD CONSTRAINT chk_xingtu_project_account_daily_video_table_id_not_blank
    CHECK (daily_video_table_id IS NULL OR btrim(daily_video_table_id) <> '');

COMMENT ON COLUMN xingtu_project_account.daily_video_table_id IS
    '项目级视频每日新增播放量宽表 ID；为空时不执行 daily video 同步';
