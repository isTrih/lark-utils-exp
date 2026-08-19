ALTER TABLE xingtu_activity_content_config
    ALTER COLUMN manual_overrides_spreadsheet SET DEFAULT true;

UPDATE xingtu_activity_content_config
SET manual_overrides_spreadsheet = true
WHERE manual_overrides_spreadsheet = false;

COMMENT ON COLUMN xingtu_activity_content_config.manual_overrides_spreadsheet IS
    '同一唯一键同时存在星图和手动登记数据时，是否手动登记覆盖星图';
