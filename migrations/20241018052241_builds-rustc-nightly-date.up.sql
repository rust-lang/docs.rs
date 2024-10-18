ALTER TABLE builds 
    ADD COLUMN rustc_nightly_date DATE;

CREATE INDEX builds_nightly_date_idx ON builds USING btree (rustc_nightly_date);

UPDATE builds 
    SET rustc_nightly_date = CAST(SUBSTRING(rustc_version FROM ' (\d+-\d+-\d+)\)$') AS DATE)
    WHERE rustc_version IS NOT NULL;
