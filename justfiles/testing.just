# just commands for CI & local development

_ensure_nextest_installed: (_ensure_cargo_installed "cargo-nextest")

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
    # sqlx prepare needs `--workspace` twice:
    # https://github.com/transact-rs/sqlx/issues/3362
    # once for the sqlx-cli, once for the inner `cargo check` command.

    cargo sqlx prepare \
        --database-url $DOCSRS_DATABASE_URL \
        --workspace \
        {{ args }} \
        -- --workspace --all-targets --all-features

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

_format_markdown mdfile:
    #!/usr/bin/env bash
    set -euo pipefail

    echo "formatting {{ mdfile }}.."
    # like this we get both the non-zero exit code, and the local code is
    # formatted.
    if ! deno fmt --quiet --check "{{ mdfile }}" >/dev/null; then
      deno fmt "{{ mdfile }}"
      exit 1
    fi

# format rust code
[group('testing')]
format-rust:
    #!/usr/bin/env bash
    set -euo pipefail

    echo "running cargo fmt"
    rustup component add rustfmt
    if ! cargo fmt --all -- --check >/dev/null; then
      echo "rust files weren't formatted..." >&2
      cargo fmt --all
      exit 1
    fi

# run all formatters, continue after error
[group('testing')]
format:
    #!/usr/bin/env bash
    set -euo pipefail

    exit_code=0

    just format-rust || exit_code=1
    just format-just || exit_code=1
    just format-markdown || exit_code=1
    just format-cargo-toml || exit_code=1

    exit "$exit_code"

# format all justfiles
[group('testing')]
format-just:
    fd \
      --type file \
      --threads 1 \
      '^(Justfile|.+\.just)$' . \
      --exec just _format_justfile {} || exit_code=1

# format all markdown files we maintain
[group('testing')]
format-markdown:
    fd \
      --type file \
      --threads 1 \
      --extension md \
      --exclude 'crates/lib/font-awesome-as-a-crate/' \
      --exclude 'crates/bin/docs_rs_web/assets/syntaxes/' \
      --exclude 'crates/bin/docs_rs_web/static/' \
      --exec just _format_markdown {} || exit_code=1

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

[group('testing')]
lint-rust:
    #!/usr/bin/env bash
    set -euo pipefail

    if [ "${GITHUB_ACTIONS:-}" = "true" ]; then
      just clippy
    else
      just clippy-fix
    fi

# run all linters, for local development & CI
[group('testing')]
lint: lint-rust lint-actions lint-js lint-dependencies-machete

# validate GitHub Actions workflows
[group('testing')]
lint-actions:
    @command -v actionlint >/dev/null || { echo "warning: actionlint not installed, skipping"; exit 0; }
    actionlint -ignore SC2086

[group('testing')]
format-cargo-toml *args:
    #!/usr/bin/env bash
    set -euo pipefail

    if [ {{ semver_matches(trim_start_match(shell("cargo-sort --version"), "cargo-sort "), ">=2.1.3") }} != 'true' ]; then
      echo "cargo-sort to old, must be >=2.1.3"
      exit 1;
    fi

    # like this we get both the non-zero exit code, and the local code is
    # formatted.
    if ! cargo sort --workspace --check {{ args }} >/dev/null; then
      cargo sort --workspace {{ args }}
      exit 1
    fi

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

_build-test-binaries: _ensure_nextest_installed
    #!/usr/bin/env bash
    set -euo pipefail

    export SQLX_OFFLINE=1
    cargo nextest run --no-run --workspace --locked

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

    cargo nextest run --workspace --locked --no-fail-fast

    # run doctests
    cargo test --doc --workspace --locked

[group('builder')]
[group('testing')]
run-builder-tests: _ensure_db_and_s3_are_running _build-test-binaries
    #!/usr/bin/env bash
    set -euo pipefail

    export DOCSRS_INCLUDE_DEFAULT_TARGETS=true
    export DOCSRS_DOCKER_IMAGE=ghcr.io/rust-lang/crates-build-env/linux-micro
    export DOCSRS_LOG=docs_rs=debug,rustwide=info
    export RUST_BACKTRACE=1
    export DOCSRS_PREFIX=ignored/cratesfyi-prefix

    cargo nextest run --locked --run-ignored ignored --test-threads=1
