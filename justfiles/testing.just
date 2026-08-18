# just commands for CI & local development

[group('testing')]
bench-rustdoc-page host="http://127.0.0.1:8888":
  rm -rf ignored/cratesfyi-prefix/archive_cache/*
  ab \
    -n 10000 \
    -c 500 \
    {{ host }}/rayon/1.11.0/rayon/

# update sqlx metadata offline mode, for all
# crates that have it.
[group('testing')]
[group('sqlx')]
sqlx-prepare *args: _ensure_db_and_s3_are_running
  # sqlx prepare needs `--workspace` twice:
  # https://github.com/transact-rs/sqlx/issues/3362
  # once for the sqlx-cli, once for the inner `cargo check` command.

  cargo sqlx prepare \
      --database-url $DOCSRS_DATABASE_URL \
      --workspace \
      {{ args }} \
      -- --workspace --all-targets --all-features

[group('testing')]
[group('sqlx')]
sqlx-check:
  just sqlx-prepare --check

[group('testing')]
[group('sqlx')]
sqlx-migrate-run: _ensure_db_and_s3_are_running
  cargo sqlx migrate run \
    --database-url $DOCSRS_DATABASE_URL \
    --source ./crates/lib/docs_rs_database/migrations

[group('testing')]
[group('sqlx')]
sqlx-migrate-revert target="0": _ensure_db_and_s3_are_running
  # --target 0 means "revert everything"
  cargo sqlx migrate revert \
    --database-url $DOCSRS_DATABASE_URL \
    --source ./crates/lib/docs_rs_database/migrations \
    --target-version {{ target }}

_format_justfile justfile:
    #!/usr/bin/env bash
    set -euo pipefail

    echo "formatting {{ justfile }}.."
    # like this we get both the non-zero exit code, and the local code is
    # formatted.
    if ! just --fmt --justfile "{{ justfile }}" --check >/dev/null; then
      just --fmt --justfile "{{ justfile }}"
      exit 1
    fi

_format_rust:
    #!/usr/bin/env bash
    set -euo pipefail

    echo "running cargo fmt"
    rustup component add rustfmt
    if ! cargo fmt --all -- --check >/dev/null; then
      echo "rust files weren't formatted..." >&2
      cargo fmt --all
      exit 1
    fi

# Format the code using `cargo fmt` and `just --fmt`.
#
# Will continue with other formatters when some fail.
[group('testing')]
format:
    #!/usr/bin/env bash
    set -euo pipefail

    exit_code=0

    just _format_rust || exit_code=1

    fd \
      --type file \
      --threads 1 \
      '^(Justfile|.+\.just)$' . \
      --exec just _format_justfile {} || exit_code=1

    exit "$exit_code"

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
lint: format lint-dependencies-machete
  #!/usr/bin/env bash
  set -euo pipefail

  if [ "${GITHUB_ACTIONS:-}" = "true" ]; then
    just sort-cargo-toml --check
    just clippy
  else
    just sort-cargo-toml
    just clippy-fix
  fi

[group('testing')]
sort-cargo-toml *args:
  #!/usr/bin/env bash
  set -euo pipefail

  if [ {{semver_matches(trim_start_match(shell("cargo-sort --version"), "cargo-sort "), ">=2.1.3")}} != 'true' ]; then
    echo "cargo-sort to old, must be >=2.1.3"
    exit 1;
  fi

  cargo sort --workspace {{ args }}


[group('testing')]
lint-dependencies-machete:
  #!/usr/bin/env bash
  set -euo pipefail

  # check unused deps with cargo machete is fast,
  # but has many false positives.
  # While it can also recurse into subdirs, it would
  # also stumble onto our broken crates in `/tests/crates/*`,
  # where I also don't want to add machete-metadata.
  fd \
    --glob 'Cargo.toml' \
    --type file \
    --exclude tests \
    -X cargo machete {}

[group('testing')]
lint-dependencies-udeps:
  #!/usr/bin/env bash
  set -euo pipefail

  # check unused deps with cargo udeps.
  # Optional, since udeps is much slower because it
  # needs to additionally compile the workspace with nightly
  echo "check for unused normal dependencies"
  cargo +nightly udeps --workspace

  echo "check for unused dev dependencies"
  cargo +nightly udeps --workspace --all-targets

[group('testing')]
lint-js *args:
  deno run -A npm:eslint@9 \
    crates/bin/docs_rs_web/static \
    crates/bin/docs_rs_web/templates \
    eslint.config.js \
    gui-tests \
    {{ args }}

[group('testing')]
run-gui-tests: _ensure_db_and_s3_are_running cli-db-migrate compose-up-web
  just cli-build-update-toolchain
  just cli-build-crate sysinfo 0.23.4
  just cli-build-crate sysinfo 0.23.5
  just cli-build-crate libtest 0.0.1
  just cli-build-crate zbus 5.15.0
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
