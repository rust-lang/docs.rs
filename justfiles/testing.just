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
  #!/usr/bin/env bash
  set -euo pipefail

  cargo sqlx prepare \
      --database-url $DOCSRS_DATABASE_URL \
      --workspace \
      {{ args }} \
      -- --all-targets --all-features

  for subcrate in crates/lib/* crates/bin/*; do
    echo "Preparing SQLx metadata for ${subcrate}..."
    # NOTE: potential optimization: only run this command when `sqlx` 
    # is in the dependencies of the subcrate.
    (cd "${subcrate}" && cargo sqlx prepare \
        --database-url $DOCSRS_DATABASE_URL \
        {{ args }} \
        -- --all-targets --all-features)
  done


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
  # for now manually, `cargo sort --workspace` is broken right now for docs.rs
  # https://github.com/DevinR528/cargo-sort/pull/121
  cargo sort {{ args }} \
    ./Cargo.toml \
    ./crates/bin/* \
    ./crates/lib/*


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
