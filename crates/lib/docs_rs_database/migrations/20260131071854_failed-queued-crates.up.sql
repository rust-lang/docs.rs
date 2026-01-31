-- Add up migration script here
DELETE FROM queue WHERE attempt >= 5;
