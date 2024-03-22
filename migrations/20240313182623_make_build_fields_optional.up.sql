ALTER TABLE builds 
    ALTER COLUMN rustc_version DROP NOT NULL, 
    ALTER COLUMN docsrs_version DROP NOT NULL, 
    ALTER COLUMN build_time DROP NOT NULL, 
    ALTER COLUMN build_time DROP DEFAULT, 
    ALTER COLUMN build_server DROP NOT NULL, 
    ALTER COLUMN build_server DROP DEFAULT;
