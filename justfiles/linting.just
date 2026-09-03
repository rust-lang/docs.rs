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

# Format all Rust source files.
[group('linting')]
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

# Run every formatter and report all formatting failures.
[group('linting')]
format:
    #!/usr/bin/env bash
    set -euo pipefail

    exit_code=0

    just format-rust || exit_code=1
    just format-just || exit_code=1
    just format-markdown || exit_code=1
    just format-cargo-toml || exit_code=1

    exit "$exit_code"

# Format all Justfiles.
[group('linting')]
format-just:
    fd \
      --type file \
      --threads 1 \
      '^(Justfile|.+\.just)$' . \
      --exec just _format_justfile {} || exit_code=1

# Format maintained Markdown files with Deno.
[group('linting')]
format-markdown:
    fd \
      --type file \
      --threads 1 \
      --extension md \
      --exclude 'crates/lib/font-awesome-as-a-crate/' \
      --exclude 'crates/bin/docs_rs_web/assets/syntaxes/' \
      --exclude 'crates/bin/docs_rs_web/static/' \
      --exec just _format_markdown {} || exit_code=1

# Run Clippy for the complete workspace.
[group('linting')]
clippy *args:
    rustup component add clippy
    cargo clippy \
      --all-features \
      --all-targets \
      --workspace \
      --locked \
      {{ args }} \
      -- -D warnings

# Apply Clippy suggestions to dirty or staged files.
[group('linting')]
clippy-fix: (clippy "--fix --allow-dirty --allow-staged")

# Run Rust linting, applying fixes outside CI.
[group('linting')]
lint-rust:
    #!/usr/bin/env bash
    set -euo pipefail

    if [ "${GITHUB_ACTIONS:-}" = "true" ]; then
      just clippy
    else
      just clippy-fix
    fi

# Run the standard local and CI lint suite.
[group('linting')]
lint: lint-rust lint-actions lint-js lint-dependencies-machete lint-mise-lock

# Verify that mise.lock matches the tools pinned in mise.toml.
[group('linting')]
lint-mise-lock:
    mise lock --upgrade --jobs=1
    git diff --exit-code -- mise.lock

# Validate GitHub Actions workflows with actionlint when installed.
[group('linting')]
lint-actions:
    @command -v actionlint >/dev/null || { echo "warning: actionlint not installed, skipping"; exit 0; }
    actionlint -ignore SC2086

# Format workspace Cargo.toml files with cargo-sort.
[group('linting')]
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

# Check workspace manifests for unused dependencies with cargo-machete.
[group('linting')]
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

# Run the slower nightly unused-dependency checks with cargo-udeps.
[group('linting')]
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

# Lint frontend JavaScript and templates with ESLint.
[group('linting')]
lint-js *args:
    deno run -A npm:eslint@9 \
      crates/bin/docs_rs_web/static \
      crates/bin/docs_rs_web/templates \
      eslint.config.js \
      gui-tests \
      {{ args }}
