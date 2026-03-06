ALTER TABLE repositories ADD COLUMN crate_count INT NOT NULL DEFAULT 0;
CREATE INDEX repositories_name_idx ON repositories USING btree (name);
