CREATE TYPE owner_kind AS ENUM (
    'user',
    'team'
);

ALTER TABLE owners
ADD COLUMN IF NOT EXISTS kind owner_kind NOT NULL DEFAULT 'user';

UPDATE owners
SET
    kind = CASE
        WHEN login LIKE 'github:%' THEN 'team'::owner_kind
        ELSE 'user'::owner_kind
    END;
