ALTER TABLE builds
    ADD COLUMN build_started TIMESTAMP WITH TIME ZONE;

ALTER TABLE builds
    RENAME COLUMN build_time TO build_finished;
