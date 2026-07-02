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
[group('sqlx')]
[group('testing')]
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

[group('sqlx')]
[group('testing')]
sqlx-check:
    just sqlx-prepare --check

[group('sqlx')]
[group('testing')]
sqlx-migrate-run: _ensure_db_and_s3_are_running
    cargo sqlx migrate run \
      --database-url $DOCSRS_DATABASE_URL \
      --source ./crates/lib/docs_rs_database/migrations

[group('sqlx')]
[group('testing')]
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
    #!/usr/bin/env bash
    set -euo pipefail

    if [ {{ semver_matches(trim_start_match(shell("cargo-sort --version"), "cargo-sort "), ">=2.1.3") }} != 'true' ]; then 
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

_gui-tests_env: _ensure_db_and_s3_are_running cli-db-migrate 
    just cli-build-update-toolchain
    just cli-build-crate sysinfo 0.23.4
    just cli-build-crate sysinfo 0.23.5
    just cli-build-crate libtest 0.0.1
    just cli-build-crate zbus 5.15.0
    just cli-build-add-essential-files

[group('testing')]
run-gui-tests: _gui-tests_env compose-up-web
    #!/usr/bin/env bash
    set -euo pipefail

    DOCSRS_CLI_MODE=docker \ 
      just _cli gui_tests 

[group('testing')]
run-gui-tests-local: # _gui-tests_env
    #!/usr/bin/env bash
    set -euo pipefail

    cargo build -p docs_rs_web

    web_pid=""

    cleanup() {
      if [ -n "$web_pid" ]; then
        kill "$web_pid" 2>/dev/null || true
        wait "$web_pid" 2>/dev/null || true
      fi
    }
    trap cleanup EXIT INT TERM

    cargo run -p docs_rs_web &
    web_pid=$!

    # Fail instead of waiting forever if the server fails to start.
    for _ in {1..100}; do
      if curl --silent --fail http://localhost:3000/ >/dev/null; then
        break
      fi
      if ! kill -0 "$web_pid" 2>/dev/null; then
        wait "$web_pid"
      fi
      sleep 0.2
    done

    curl --silent --fail http://localhost:3000/ >/dev/null

    npm install --loglevel verbose --force
    npx --no-install puppeteer browsers install chrome

    node ./gui-tests/tester.js


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

[group('builder')]
[group('testing')]
run-builder-tests: _ensure_db_and_s3_are_running
    #!/usr/bin/env bash
    set -euo pipefail

    export DOCSRS_INCLUDE_DEFAULT_TARGETS=true
    export DOCSRS_DOCKER_IMAGE=ghcr.io/rust-lang/crates-build-env/linux-micro
    export DOCSRS_LOG=docs_rs=debug,rustwide=info
    export RUST_BACKTRACE=1
    export DOCSRS_PREFIX=ignored/cratesfyi-prefix

    cargo test --locked -- --ignored --test-threads=1
