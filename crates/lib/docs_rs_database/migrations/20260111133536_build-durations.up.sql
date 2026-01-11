CREATE VIEW build_durations AS

SELECT
    b.id AS build_id,
    CASE
        -- for old builds, `build_started` is empty.
        WHEN b.build_started IS NULL
            THEN NULL
        ELSE
            CASE
                -- for in-progress builds we show the duration until now
                WHEN b.build_finished IS NULL
                    THEN (CURRENT_TIMESTAMP - b.build_started)
                -- for finished builds we can show the full duration
                ELSE (b.build_finished - b.build_started)
            END
    END AS duration
FROM
    builds AS b
;

CREATE VIEW build_durations_release AS

SELECT
    r.crate_id,
    b.rid,
    AVG(bd.duration) AS avg_duration

FROM
    releases AS r
INNER JOIN builds AS b ON r.id = b.rid
INNER JOIN build_durations AS bd ON b.id = bd.build_id

WHERE
    b.build_status = 'success' AND
    b.build_started IS NOT NULL

GROUP BY r.crate_id, b.rid
;

CREATE VIEW build_durations_crate AS

SELECT
    r.crate_id,
    AVG(bd.duration) AS avg_duration

FROM
    releases AS r
INNER JOIN builds AS b ON r.id = b.rid
INNER JOIN build_durations AS bd ON b.id = bd.build_id

WHERE
    b.build_status = 'success' AND
    b.build_started IS NOT NULL

GROUP BY r.crate_id
;
