ALTER TABLE releases ADD COLUMN build_status BOOL;

UPDATE releases SET build_status = (
    SELECT builds.build_status
    FROM builds
    WHERE builds.rid = releases.id
    ORDER BY builds.build_time DESC
    LIMIT 1
);
