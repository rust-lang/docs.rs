ALTER TABLE owners
ADD COLUMN IF NOT EXISTS kind TEXT NOT NULL CHECK (
    kind IN ('user', 'team')
) DEFAULT 'user';
