set shell := ["bash", "-Eeuo", "pipefail", "-c"]
set ignore-comments
set unstable
set lists
set dotenv-load
set dotenv-override

# minimal settings to run justfile recipes that don't just run docker, like `run-tests`.
# Typically you will want to create your own `.env` file based on `.env.sample` for
# easier local development.
export DOCSRS_PREFIX := env("DOCSRS_PREFIX", "ignored/cratesfyi-prefix")
export DOCSRS_POSTGRES_PORT := env("DOCSRS_POSTGRES_PORT", "15432")
export DOCSRS_DATABASE_URL := env("DOCSRS_DATABASE_URL", "postgresql://cratesfyi:password@localhost:" + DOCSRS_POSTGRES_PORT)
export DATABASE_URL := env("DATABASE_URL", DOCSRS_DATABASE_URL)
export AWS_ACCESS_KEY_ID := env("AWS_ACCESS_KEY_ID", "cratesfyi")
export AWS_SECRET_ACCESS_KEY := env("AWS_SECRET_ACCESS_KEY", "secret_key")
export DOCSRS_S3_PORT := env("DOCSRS_S3_PORT", "9000")
export DOCSRS_S3_CONSOLE_PORT := env("DOCSRS_S3_CONSOLE_PORT", "9001")
export S3_ENDPOINT := env("S3_ENDPOINT", "http://localhost:" + DOCSRS_S3_PORT)

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
