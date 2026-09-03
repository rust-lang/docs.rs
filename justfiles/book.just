_ensure_mdbook_installed: (_ensure_cargo_installed "mdbook") (_ensure_cargo_installed "mdbook-linkcheck2") (_ensure_cargo_installed "mdbook-mermaid")

# Generate CLI help pages and build the developer guide.
[group('book')]
book-build *args: _ensure_mdbook_installed
    #!/usr/bin/env bash
    set -euo pipefail

    export SQLX_OFFLINE=1
    mkdir -p docs/src/generated/
    for subcommand in "" "build" "database" "queue" "cdn" ; do
      echo "generating help output for Admin CLI ($subcommand) subcommand"
      cargo run --bin docs_rs_admin -- $subcommand --help \
        > docs/src/generated/docs_rs_admin-"$subcommand"-help.txt
    done

    mdbook build docs {{ args }}

# Build and test the developer guide.
[group('book')]
[working-directory('./docs/')]
book-test: book-build
    mdbook test

# Build the developer guide and open it in a browser.
[group('book')]
book-open: (book-build "--open")

# Watch, rebuild, and serve the developer guide locally.
[group('book')]
book-watch: book-build
    mdbook watch ./docs --open
