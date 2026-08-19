ALTER TABLE xingtu_activity_content_config
    ALTER COLUMN manual_overrides_spreadsheet SET DEFAULT false;

UPDATE xingtu_activity_content_config
SET manual_overrides_spreadsheet = false
WHERE manual_overrides_spreadsheet = true;

COMMENT ON COLUMN xingtu_activity_content_config.manual_overrides_spreadsheet IS
    '同一唯一键同时存在星图和手动登记数据时是否由手动登记覆盖；默认 false，优先使用星图数据';
