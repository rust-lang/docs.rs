DROP INDEX builds_nightly_date_idx;

ALTER TABLE builds 
    DROP COLUMN rustc_nightly_date;
