-- Add up migration script here
DO $$
DECLARE
    partition_start DATE := DATE '2025-06-01';
    partition_end_exclusive DATE := DATE '2027-07-01';
    current_month DATE;
    next_month DATE;
    partition_name TEXT;
BEGIN
    current_month := partition_start;

    WHILE current_month < partition_end_exclusive LOOP
        next_month := current_month + INTERVAL '1 month';
        partition_name := 'video_daily_metric_' || to_char(current_month, 'YYYY_MM');

        EXECUTE format(
            'CREATE TABLE IF NOT EXISTS %I PARTITION OF video_daily_metric FOR VALUES FROM (%L) TO (%L)',
            partition_name,
            current_month,
            next_month
        );

        current_month := next_month;
    END LOOP;
END $$;
