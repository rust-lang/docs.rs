DELETE FROM builds WHERE rid in (
    SELECT id FROM releases WHERE (
        release_time IS NULL OR
        target_name IS NULL OR
        yanked IS NULL OR
        is_library IS NULL OR
        rustdoc_status IS NULL OR
        have_examples IS NULL OR
        downloads IS NULL OR
        doc_targets IS NULL OR
        default_target IS NULL OR
        archive_storage IS NULL
    )
);

DELETE FROM releases WHERE (
    release_time IS NULL OR
    target_name IS NULL OR
    yanked IS NULL OR
    is_library IS NULL OR
    rustdoc_status IS NULL OR
    have_examples IS NULL OR
    downloads IS NULL OR
    doc_targets IS NULL OR
    default_target IS NULL OR
    archive_storage IS NULL
);

ALTER TABLE releases
    ALTER COLUMN release_time SET NOT NULL,
    ALTER COLUMN target_name SET NOT NULL,
    ALTER COLUMN yanked SET NOT NULL,
    ALTER COLUMN yanked SET DEFAULT false,
    ALTER COLUMN is_library SET NOT NULL,
    ALTER COLUMN is_library SET DEFAULT true,
    ALTER COLUMN rustdoc_status SET NOT NULL,
    ALTER COLUMN rustdoc_status SET DEFAULT false,
    ALTER COLUMN test_status SET DEFAULT false,
    ALTER COLUMN have_examples SET NOT NULL,
    ALTER COLUMN have_examples SET DEFAULT false,
    ALTER COLUMN downloads SET NOT NULL,
    ALTER COLUMN downloads SET DEFAULT 0,
    ALTER COLUMN doc_targets SET NOT NULL,
    ALTER COLUMN doc_targets SET DEFAULT '[]'::json,
    ALTER COLUMN default_target SET NOT NULL;
