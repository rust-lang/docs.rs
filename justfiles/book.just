_ensure_mdbook_installed: (_ensure_cargo_installed "mdbook" "mdbook-linkcheck2" "mdbook-mermaid")

[group('book')]
book-build *args: _ensure_mdbook_installed
    #!/usr/bin/env bash
    set -euo pipefail

    export SQLX_OFFLINE=1
    mkdir -p docs/src/generated/
    for subcommand in "" "build" "database" "queue" "cdn" ; do
      echo "generating help output for admin CLI ($subcommand) subdommand"
      cargo run --bin docs_rs_admin -- $subcommand --help \
        > docs/src/generated/docs_rs_admin-"$subcommand"-help.txt
    done

    mdbook build docs {{ args }}

[group('book')]
[working-directory('./docs/')]
book-test: book-build
    mdbook test

[group('book')]
book-open: (book-build "--open")

[group('book')]
book-watch: book-build
    mdbook watch ./docs --open
