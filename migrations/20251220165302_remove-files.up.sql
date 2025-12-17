-- on the off-chance that someone is self-hosting docs.rs, and 
-- using database-storage, and is using `sqlx migrate run` instead of our 
-- own migrate-subcommand, 

-- we shouldn't drop the files.

-- DROP TABLE files;

