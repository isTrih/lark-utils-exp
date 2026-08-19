UPDATE live_session
SET viewer_pv = live_exposure_pv
WHERE viewer_pv IS NULL
  AND live_exposure_pv IS NOT NULL;

COMMENT ON COLUMN live_session.viewer_pv IS '观看人次 PV；当前星图直播来源字段为直播曝光pv';
