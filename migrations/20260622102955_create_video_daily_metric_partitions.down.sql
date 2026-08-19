-- Add down migration script here
DO $$
DECLARE
    partition_start DATE := DATE '2025-06-01';
    partition_end_exclusive DATE := DATE '2027-07-01';
    current_month DATE;
    partition_name TEXT;
BEGIN
    current_month := partition_start;

    WHILE current_month < partition_end_exclusive LOOP
        partition_name := 'video_daily_metric_' || to_char(current_month, 'YYYY_MM');

        EXECUTE format(
            'DROP TABLE IF EXISTS %I',
            partition_name
        );

        current_month := current_month + INTERVAL '1 month';
    END LOOP;
END $$;
