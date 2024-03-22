DELETE FROM builds WHERE (
    rustc_version IS NULL OR 
    docsrs_version IS NULL OR 
    build_time IS NULL OR 
    build_server IS NULL
);


ALTER TABLE builds 
    ALTER COLUMN rustc_version SET NOT NULL, 
    ALTER COLUMN docsrs_version SET NOT NULL, 
    ALTER COLUMN build_time SET NOT NULL, 
    ALTER COLUMN build_time SET DEFAULT now(),
    ALTER COLUMN build_server SET NOT NULL, 
    ALTER COLUMN build_server SET DEFAULT '';
