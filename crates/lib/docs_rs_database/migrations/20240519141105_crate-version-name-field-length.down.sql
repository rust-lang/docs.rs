-- this reverse migration might fail when we have other records
-- referencing releases with longer names or versions.
-- If this has to be reverted we probably would have to manually run 
-- db::delete::delete_crate and db::delete::delete_version for the releases 
-- and crates.
DELETE FROM crates WHERE LENGTH(name) > 255;
DELETE FROM releases WHERE LENGTH(version) > 100;

ALTER TABLE crates ALTER COLUMN name TYPE VARCHAR(255);
ALTER TABLE releases ALTER COLUMN version TYPE VARCHAR(100);
