CREATE TABLE builds_logs(
    build_id INTEGER PRIMARY KEY REFERENCES builds(id),
    logs JSON
);
