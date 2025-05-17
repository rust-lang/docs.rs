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

lint *args: 
  cargo clippy --all-features --all-targets --workspace --locked {{ args }} -- -D warnings

lint-fix:
  just lint --fix --allow-dirty --allow-staged

lint-js *args:
  deno run -A npm:eslint@9 static templates gui-tests eslint.config.js {{ args }}

# Initialize the docker compose database
[group('compose')]
compose-migrate:
  docker compose run --build --rm cli database migrate

# Update last seen reference to the current index head, to only build newly published crates
[group('compose')]
compose-queue-head:
  docker compose run --build --rm cli queue set-last-seen-reference --head

# Launch base docker services, ensuring the database is migrated
[group('compose')]
compose-up:
  just compose-migrate
  docker compose up --build -d

# Launch base docker services and registry watcher, ensuring the database is migrated
[group('compose')]
compose-up-watch:
  just compose-migrate
  docker compose --profile watch up --build -d

# Shutdown docker services and cleanup all temporary volumes
[group('compose')]
compose-down:
  docker compose --profile all down --volumes --remove-orphans
