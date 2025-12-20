LOCK builds, releases;

DELETE FROM builds WHERE build_status = 'in_progress';

ALTER TABLE builds ALTER build_status
TYPE BOOL
USING build_status = 'success';

DROP TYPE BUILD_STATUS;
