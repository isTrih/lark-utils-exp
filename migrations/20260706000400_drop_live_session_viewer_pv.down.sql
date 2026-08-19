ALTER TABLE live_session
    ADD COLUMN viewer_pv BIGINT;

UPDATE live_session
SET viewer_pv = live_exposure_pv
WHERE live_exposure_pv IS NOT NULL;

ALTER TABLE live_session
    ADD CONSTRAINT chk_live_session_viewer_pv_non_negative
    CHECK (viewer_pv IS NULL OR viewer_pv >= 0);

COMMENT ON COLUMN live_session.viewer_pv IS '观看人次 PV';
