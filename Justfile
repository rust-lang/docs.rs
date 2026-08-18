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

@_ensure_mdbook_installed:
    command -v mdbook >/dev/null 2>&1 || cargo install mdbook
    command -v mdbook >/dev/null 2>&1 || cargo install mdbook-linkcheck2

[group('book')]
book-build *args: _ensure_mdbook_installed
    mdbook build docs {{ args }}

[group('book')]
[working-directory('./docs/')]
book-test: _ensure_mdbook_installed
    mdbook test

[group('book')]
book-open: (book-build "--open")
