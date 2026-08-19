ALTER TABLE xingtu_project_account
    DROP CONSTRAINT IF EXISTS chk_xingtu_project_account_daily_video_table_id_not_blank;

ALTER TABLE xingtu_project_account
    DROP COLUMN IF EXISTS daily_video_table_id;
