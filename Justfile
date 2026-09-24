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

import 'justfiles/book.just'
import 'justfiles/cli.just'
import 'justfiles/docker.just'
import 'justfiles/linting.just'
import 'justfiles/services.just'
import 'justfiles/sqlx.just'
import 'justfiles/testing.just'
import 'justfiles/utils.just'

# Open a PostgreSQL shell for the local docs.rs database.
psql: compose-up-resources
    psql $DOCSRS_DATABASE_URL
