UPDATE owners SET avatar = '' WHERE LENGTH(avatar) > 255;
ALTER TABLE owners ALTER COLUMN avatar TYPE VARCHAR(255);
