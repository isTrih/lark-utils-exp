ALTER TABLE live_session
    DROP CONSTRAINT IF EXISTS chk_live_session_viewer_pv_non_negative;

ALTER TABLE live_session
    DROP COLUMN IF EXISTS viewer_pv;
