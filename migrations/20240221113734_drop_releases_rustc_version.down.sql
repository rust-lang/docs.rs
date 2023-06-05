ALTER TABLE releases ADD COLUMN doc_rustc_version VARCHAR(100);

UPDATE releases SET doc_rustc_version = (
    SELECT builds.rustc_version
    FROM builds
    WHERE builds.rid = releases.id
    ORDER BY builds.build_time DESC
    LIMIT 1
);

ALTER TABLE releases ALTER COLUMN doc_rustc_version SET NOT NULL;
