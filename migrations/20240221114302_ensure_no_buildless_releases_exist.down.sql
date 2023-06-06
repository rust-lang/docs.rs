DELETE FROM compression_rels WHERE NOT EXISTS (
    SELECT 1
    FROM releases
    INNER JOIN builds ON builds.rid = releases.id
    WHERE releases.id = compression_rels.release
);

DELETE FROM keyword_rels WHERE NOT EXISTS (
    SELECT 1
    FROM releases
    INNER JOIN builds ON builds.rid = releases.id
    WHERE releases.id = keyword_rels.rid
);

DELETE FROM releases WHERE NOT EXISTS (
    SELECT *
    FROM builds
    WHERE builds.rid = releases.id
);
