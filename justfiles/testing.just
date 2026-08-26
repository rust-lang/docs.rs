# just commands for CI & local development

# Benchmark a rustdoc page with ApacheBench.
[group('testing')]
bench-rustdoc-page host="http://127.0.0.1:8888":
    rm -rf ignored/cratesfyi-prefix/archive_cache/*
    ab \
      -n 10000 \
      -c 500 \
      {{ host }}/rayon/1.11.0/rayon/

# Build the crate fixtures required by the GUI test suite.
[group('testing')]
prepare-gui-tests: _ensure_db_migrated
    just _builder build update-toolchain
    just _builder build crate sysinfo 0.23.4
    just _builder build crate sysinfo 0.23.5
    just _builder build crate libtest 0.0.1
    just _builder build crate zbus 5.15.0
    just _builder build add-essential-files

# Run browser assertions against an existing web server on localhost:3000.
[group('testing')]
run-gui-browser-tests:
    npm install --loglevel verbose
    npx --no-install puppeteer browsers install chrome
    node ./gui-tests/tester.js

# Run GUI tests with the web server on the host, reusing existing fixtures.
[group('testing')]
run-gui-tests: _ensure_db_migrated
    #!/usr/bin/env bash
    set -euo pipefail

    cargo build -p docs_rs_web

    web_pid=""

    cleanup() {
      if [ -n "$web_pid" ]; then
        kill -KILL "$web_pid" 2>/dev/null || true
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
    just run-gui-browser-tests

# Run GUI tests against the packaged web-server Compose service.
[group('testing')]
run-gui-tests-e2e: compose-up-web run-gui-browser-tests

_build-test-binaries:
    #!/usr/bin/env bash
    set -euo pipefail

    export SQLX_OFFLINE=1
    cargo test --no-run --workspace --locked

# Build and run the complete Rust workspace test suite.
[group('testing')]
run-tests: compose-up-resources _build-test-binaries
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

# Run ignored builder integration tests serially.
[group('testing')]
run-builder-tests: compose-up-resources
    #!/usr/bin/env bash
    set -euo pipefail

    export DOCSRS_INCLUDE_DEFAULT_TARGETS=true
    export DOCSRS_DOCKER_IMAGE=ghcr.io/rust-lang/crates-build-env/linux-micro
    export DOCSRS_LOG=docs_rs=debug,rustwide=info
    export RUST_BACKTRACE=1
    export DOCSRS_PREFIX=ignored/cratesfyi-prefix

    cargo test --locked -- --ignored --test-threads=1

# Test migrations with both SQLx CLI and docs_rs_admin.
[group('testing')]
test-database-migrations:
    just sqlx-migrate-run
    just sqlx-check
    just sqlx-migrate-revert
    just cli-db-migrate
    just cli-db-migrate 0
