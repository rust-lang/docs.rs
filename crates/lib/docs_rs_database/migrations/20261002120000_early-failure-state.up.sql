ALTER TABLE builds ADD COLUMN early_failure boolean NOT NULL DEFAULT false;

-- Preserve the previous classification; historical completion times are unknown.
UPDATE builds SET early_failure = true
WHERE build_status = 'failure' AND build_finished IS NULL;
