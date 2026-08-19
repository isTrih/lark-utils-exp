ALTER TABLE xingtu_activity_period
    DROP CONSTRAINT IF EXISTS chk_xingtu_activity_period_cpm_table_id_not_blank;

ALTER TABLE xingtu_activity_period
    DROP COLUMN IF EXISTS cpm_table_id;
