-- this reverse migration might fail when we have other records
-- referencing releases with longer license strings.
--
-- If this has to be reverted we probably would have to manually run
-- db::delete::delete_version for the releases.
DELETE FROM releases WHERE LENGTH(license) > 100;
ALTER TABLE releases ALTER COLUMN license TYPE VARCHAR(100);
