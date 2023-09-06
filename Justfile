# List available commands
_default:
    just --list

sqlx-prepare:
  cargo sqlx prepare \
    --database-url $DOCSRS_DATABASE_URL \
    --workspace \
    --check \
    -- --all-targets --all-features
