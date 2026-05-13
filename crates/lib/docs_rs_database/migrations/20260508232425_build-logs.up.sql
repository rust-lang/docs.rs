CREATE TABLE builds_logs(
    id INTEGER NOT NULL,
    build_id INTEGER REFERENCES builds(id),
    log_filename TEXT,
    success BOOLEAN
);

CREATE SEQUENCE builds_logs_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;
ALTER SEQUENCE builds_logs_id_seq OWNED BY builds_logs.id;
ALTER TABLE ONLY builds_logs ALTER COLUMN id SET DEFAULT nextval('builds_logs_id_seq'::regclass);

CREATE INDEX build_logs_build_id_idx on builds_logs(build_id);

ALTER TYPE build_status ADD VALUE IF NOT EXISTS 'partial_failure' AFTER 'failure';
