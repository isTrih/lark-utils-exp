ALTER TABLE xingtu_activity_period
    ADD COLUMN cpm_table_id TEXT;

UPDATE xingtu_activity_period
SET cpm_table_id = 'tbluqqg9N4Umt6jr'
WHERE activity_period_id = 1;

ALTER TABLE xingtu_activity_period
    ADD CONSTRAINT chk_xingtu_activity_period_cpm_table_id_not_blank
    CHECK (cpm_table_id IS NULL OR btrim(cpm_table_id) <> '');

COMMENT ON COLUMN xingtu_activity_period.cpm_table_id IS
    '本期直播/视频 CPM 配置多维表 ID；表内字段名为直播CPM、视频CPM';
