CREATE TABLE essential_files (
    rustc_version TEXT PRIMARY KEY,
    published_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

-- already published before this table existed
INSERT INTO essential_files (rustc_version)
SELECT value #>> '{}' FROM config WHERE name = 'rustc_version';
