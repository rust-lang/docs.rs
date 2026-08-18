set shell := ["bash", "-Eeuo", "pipefail", "-c"]
set ignore-comments
set dotenv-load
set dotenv-override

# minimal settings to run justfile recipes that don't just run docker, like `run-tests`.
# Typically you will want to create your own `.env` file based on `.env.sample` for
# easier local development.
export DOCSRS_PREFIX := env("DOCSRS_PREFIX", "ignored/cratesfyi-prefix")
export DOCSRS_DATABASE_URL := env("DOCSRS_DATABASE_URL", "postgresql://cratesfyi:password@localhost:15432")
export AWS_ACCESS_KEY_ID := env("AWS_ACCESS_KEY_ID", "cratesfyi")
export AWS_SECRET_ACCESS_KEY := env("AWS_SECRET_ACCESS_KEY", "secret_key")
export S3_ENDPOINT := env("S3_ENDPOINT", "http://localhost:9000")

# List available commands
_default:
    @just --list

import 'justfiles/cli.just'
import 'justfiles/utils.just'
import 'justfiles/services.just'
import 'justfiles/testing.just'

psql:
    psql $DOCSRS_DATABASE_URL

# helper recipe to ensure a CLI tool is installed. 
# * Accepts multiple names 
# * uses `cargo binstall` if it exists.
#
# example usage: 
# ```
# _ensure_mdbook_installed: (_ensure_cargo_installed "mdbook" "mdbook-linkcheck2")
# ```
_ensure_cargo_installed *packages:
    #!/usr/bin/env bash
    set -euo pipefail

    for package in {{ packages }} ; do
      if command -v "$package" >/dev/null 2>&1; then
        continue
      fi

      if command -v cargo-binstall >/dev/null 2>&1; then
        cargo binstall -y "$package"
      else
        cargo install "$package"
      fi
    done

_ensure_mdbook_installed: (_ensure_cargo_installed "mdbook" "mdbook-linkcheck2" "mdbook-mermaid")

[group('book')]
book-build *args: _ensure_mdbook_installed
    mdbook build docs {{ args }}

[group('book')]
[working-directory('./docs/')]
book-test: book-build
    mdbook test

[group('book')]
book-open: (book-build "--open")
