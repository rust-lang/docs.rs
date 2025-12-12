ALTER TABLE builds DROP COLUMN build_started;
ALTER TABLE builds RENAME COLUMN build_finished TO build_time;
