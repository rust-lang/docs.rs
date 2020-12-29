CREATE TYPE build_status AS ENUM (
    'in_progress',
    'success',
    'failure'
);

ALTER TABLE builds ALTER build_status
TYPE build_status
USING CASE
    WHEN build_status
        THEN 'success'::build_status
    ELSE 'failure'::build_status
END;
