# Regenerate SQLx offline metadata for the workspace.
[group('sqlx')]
sqlx-prepare *args: compose-up-resources
    # sqlx prepare needs `--workspace` twice:
    # https://github.com/transact-rs/sqlx/issues/3362
    # once for the sqlx-cli, once for the inner `cargo check` command.

    cargo sqlx prepare \
        --database-url $DOCSRS_DATABASE_URL \
        --workspace \
        {{ args }} \
        -- --workspace --all-targets --all-features

# Check that committed SQLx offline metadata is current.
[group('sqlx')]
sqlx-check: (sqlx-prepare "--check")

# Apply migrations and regenerate SQLx offline metadata.
[group('sqlx')]
sqlx-update: sqlx-migrate-run sqlx-prepare

# Apply all pending SQLx migrations to the local database.
[group('sqlx')]
sqlx-migrate-run: compose-up-resources
    cargo sqlx migrate run \
      --database-url $DOCSRS_DATABASE_URL \
      --source ./crates/lib/docs_rs_database/migrations

# Revert SQLx migrations down to a target version (defaults to zero).
[group('sqlx')]
sqlx-migrate-revert target="0": compose-up-resources
    # --target 0 means "revert everything"
    cargo sqlx migrate revert \
      --database-url $DOCSRS_DATABASE_URL \
      --source ./crates/lib/docs_rs_database/migrations \
      --target-version {{ target }}
