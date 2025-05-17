# List available commands
_default:
    just --list

sqlx-prepare ADDITIONAL_ARGS="":
  cargo sqlx prepare \
    --database-url $DOCSRS_DATABASE_URL \
    --workspace {{ ADDITIONAL_ARGS }} \
    -- --all-targets --all-features

sqlx-check:
  just sqlx-prepare "--check"

lint: 
  cargo clippy --all-features --all-targets --workspace --locked -- -D warnings

lint-js *args:
  deno run -A npm:eslint@9 static templates gui-tests eslint.config.js {{ args }}
