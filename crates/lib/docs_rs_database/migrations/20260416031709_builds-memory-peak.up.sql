ALTER TABLE builds ADD COLUMN memory_peak BIGINT;
CREATE INDEX builds_memory_peak_idx ON builds USING btree (memory_peak) ;
