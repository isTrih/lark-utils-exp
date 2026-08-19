CREATE FUNCTION normalize_audit_extra_field_value(input_value JSONB)
RETURNS JSONB
LANGUAGE SQL
IMMUTABLE
PARALLEL SAFE
AS $function$
    SELECT CASE
        WHEN jsonb_typeof(input_value) = 'object'
            AND jsonb_typeof(input_value -> 'type') = 'number'
            AND input_value ? 'value'
        THEN CASE
            WHEN jsonb_typeof(input_value -> 'value') = 'array'
                AND jsonb_array_length(input_value -> 'value') > 0
                AND NOT EXISTS (
                    SELECT 1
                    FROM jsonb_array_elements(input_value -> 'value') AS segment(item)
                    WHERE jsonb_typeof(segment.item) IS DISTINCT FROM 'object'
                        OR jsonb_typeof(segment.item -> 'text') IS DISTINCT FROM 'string'
                        OR (
                            segment.item ? 'type'
                            AND segment.item ->> 'type' <> 'text'
                        )
                )
            THEN to_jsonb((
                SELECT string_agg(segment.item ->> 'text', '' ORDER BY segment.ordinality)
                FROM jsonb_array_elements(input_value -> 'value')
                    WITH ORDINALITY AS segment(item, ordinality)
            ))
            ELSE input_value -> 'value'
        END
        ELSE input_value
    END
$function$;

WITH normalized AS (
    SELECT
        video.content_config_id,
        video.video_id,
        (
            SELECT jsonb_object_agg(
                entry.key,
                normalize_audit_extra_field_value(entry.value)
            )
            FROM jsonb_each(video.audit_extra) AS entry(key, value)
        ) AS audit_extra
    FROM video_content AS video
    WHERE EXISTS (
        SELECT 1
        FROM jsonb_each(video.audit_extra) AS entry(key, value)
        WHERE normalize_audit_extra_field_value(entry.value) IS DISTINCT FROM entry.value
    )
)
UPDATE video_content AS video
SET audit_extra = normalized.audit_extra
FROM normalized
WHERE video.content_config_id = normalized.content_config_id
    AND video.video_id = normalized.video_id;

WITH normalized AS (
    SELECT
        live.content_config_id,
        live.live_room_id,
        (
            SELECT jsonb_object_agg(
                entry.key,
                normalize_audit_extra_field_value(entry.value)
            )
            FROM jsonb_each(live.audit_extra) AS entry(key, value)
        ) AS audit_extra
    FROM live_session AS live
    WHERE EXISTS (
        SELECT 1
        FROM jsonb_each(live.audit_extra) AS entry(key, value)
        WHERE normalize_audit_extra_field_value(entry.value) IS DISTINCT FROM entry.value
    )
)
UPDATE live_session AS live
SET audit_extra = normalized.audit_extra
FROM normalized
WHERE live.content_config_id = normalized.content_config_id
    AND live.live_room_id = normalized.live_room_id;

COMMENT ON FUNCTION normalize_audit_extra_field_value(JSONB) IS
    '移除飞书数字 type + value 传输包装；富文本片段数组按顺序拼接为字符串';
COMMENT ON COLUMN video_content.audit_extra IS
    '审核扩展字段；飞书传输包装已移除，保留字符串、数字、布尔及其他业务 JSON 类型';
COMMENT ON COLUMN live_session.audit_extra IS
    '审核扩展字段；飞书传输包装已移除，保留字符串、数字、布尔及其他业务 JSON 类型';
