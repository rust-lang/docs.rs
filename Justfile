# List available commands
_default:
    just --list

sqlx-prepare:
  cargo sqlx prepare \
    --database-url $DOCSRS_DATABASE_URL \
    --workspace \
    -- --all-targets --all-features
