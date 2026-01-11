ALTER TABLE builds ADD PRIMARY KEY (id);
CREATE INDEX builds_build_started_idx ON builds USING btree (build_started);
