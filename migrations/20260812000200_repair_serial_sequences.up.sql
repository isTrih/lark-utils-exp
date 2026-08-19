DO $$
DECLARE
    serial_column RECORD;
    max_value BIGINT;
BEGIN
    FOR serial_column IN
        SELECT
            columns.table_schema,
            columns.table_name,
            columns.column_name,
            pg_get_serial_sequence(
                format('%I.%I', columns.table_schema, columns.table_name),
                columns.column_name
            ) AS sequence_name
        FROM information_schema.columns AS columns
        WHERE columns.table_schema = 'public'
          AND columns.column_default LIKE 'nextval(%'
    LOOP
        EXECUTE format(
            'SELECT MAX(%I)::BIGINT FROM %I.%I',
            serial_column.column_name,
            serial_column.table_schema,
            serial_column.table_name
        ) INTO max_value;

        PERFORM setval(
            serial_column.sequence_name,
            GREATEST(COALESCE(max_value, 1), 1),
            max_value IS NOT NULL
        );
    END LOOP;
END
$$;
