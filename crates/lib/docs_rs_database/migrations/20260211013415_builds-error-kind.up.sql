ALTER TABLE builds ADD COLUMN error_kind TEXT;
CREATE INDEX builds_error_kind_idx ON builds USING btree (error_kind) ;
