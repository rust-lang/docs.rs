# just commands for CI & local development

[group('testing')]
[group('sqlx')]
sqlx-prepare *args: _ensure_db_and_s3_are_running
  cargo sqlx prepare \
    --database-url $DOCSRS_DATABASE_URL \
    --workspace {{ args }} \
    -- --all-targets --all-features

[group('testing')]
[group('sqlx')]
sqlx-check:
  just sqlx-prepare --check

[group('testing')]
[group('sqlx')]
sqlx-migrate-run: _ensure_db_and_s3_are_running
  cargo sqlx migrate run --database-url $DOCSRS_DATABASE_URL

[group('testing')]
[group('sqlx')]
sqlx-migrate-revert target="0": _ensure_db_and_s3_are_running
  # --target 0 means "revert everything"
  cargo sqlx migrate revert \
    --database-url $DOCSRS_DATABASE_URL \
    --target-version {{ target }}


# Format the code using `cargo fmt`. 
[group('testing')]
format:
    rustup component add rustfmt
    # like this we get both the non-zero exit code, and the local code is 
    # formatted.
    cargo fmt --all -- --check || { cargo fmt --all && exit 1; }

# run clippy, in our config
[group('testing')]
clippy *args:
  rustup component add clippy
  cargo clippy \
    --all-features \
    --all-targets \
    --workspace \
    --locked \
    {{ args }} \
    -- -D warnings 

# run clippy --fix
[group('testing')]
clippy-fix:
    just clippy --fix --allow-dirty --allow-staged

# run all linters, for local development & CI
[group('testing')]
lint: format 
  #!/usr/bin/env bash
  set -euo pipefail

  if [ "${GITHUB_ACTIONS:-}" = "true" ]; then
    just clippy
  else
    just clippy-fix
  fi


[group('testing')]
lint-js *args:
  deno run -A npm:eslint@9 static templates gui-tests eslint.config.js {{ args }}
  
[group('testing')]
run-gui-tests: _ensure_db_and_s3_are_running cli-db-migrate compose-up-web
  just cli-build-update-toolchain
  just cli-build-crate sysinfo 0.23.4
  just cli-build-crate sysinfo 0.23.5
  just cli-build-crate libtest 0.0.1
  just cli-build-add-essential-files

  just _cli gui_tests 

_build-test-binaries:
  #!/usr/bin/env bash
  set -euo pipefail

  export SQLX_OFFLINE=1
  cargo test --no-run --workspace --locked

[group('testing')]
run-tests: _ensure_db_and_s3_are_running _build-test-binaries
  #!/usr/bin/env bash
  set -euo pipefail

  export DOCSRS_INCLUDE_DEFAULT_TARGETS=false
  export DOCSRS_LOG=docs_rs=debug,rustwide=info
  export RUST_BACKTRACE=1
  export DOCSRS_PREFIX=ignored/cratesfyi-prefix
  export DOCSRS_DATABASE_URL=postgresql://cratesfyi:password@localhost:15432
  export AWS_ACCESS_KEY_ID=cratesfyi
  export AWS_SECRET_ACCESS_KEY=secret_key
  export S3_ENDPOINT=http://localhost:9000

  cargo test --workspace --locked --no-fail-fast

[group('testing')]
[group('builder')]
run-builder-tests: _ensure_db_and_s3_are_running
  #!/usr/bin/env bash
  set -euo pipefail

  export DOCSRS_INCLUDE_DEFAULT_TARGETS=true
  export DOCSRS_DOCKER_IMAGE=ghcr.io/rust-lang/crates-build-env/linux-micro
  export DOCSRS_LOG=docs_rs=debug,rustwide=info
  export RUST_BACKTRACE=1
  export DOCSRS_PREFIX=ignored/cratesfyi-prefix

  cargo test --locked -- --ignored --test-threads=1
