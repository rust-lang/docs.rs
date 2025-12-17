-- CREATE TABLE files (
--     path character varying(4096) NOT NULL,
--     mime character varying(100) NOT NULL,
--     date_updated timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL,
--     content bytea,
--     compression integer
-- );

-- ALTER TABLE ONLY files
--     ADD CONSTRAINT files_pkey PRIMARY KEY (path);
