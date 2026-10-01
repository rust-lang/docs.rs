-- Restore the timestamp-based classification used by the previous API.
UPDATE builds SET build_finished = NULL WHERE early_failure;
ALTER TABLE builds DROP COLUMN early_failure;
