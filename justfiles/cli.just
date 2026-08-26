# Builder commands default to the host on amd64 Linux and Docker elsewhere.
builder_cli_mode_default := if os() + "-" + arch() == "linux-x86_64" { "local" } else { "docker" }
export DOCSRS_BUILDER_CLI_MODE := env("DOCSRS_BUILDER_CLI_MODE", builder_cli_mode_default)

# Admin and watcher commands default to the host on every platform.
export DOCSRS_CLI_MODE := env("DOCSRS_CLI_MODE", "local")

_run-cli mode package service_name *args:
    #!/usr/bin/env bash
    set -euo pipefail

    case "{{ mode }}" in
      local)
        cargo run -p {{ package }} -- {{ args }}
        ;;
      docker)
        just docker-run {{ service_name }} {{ args }}
        ;;
      *)
        echo "Invalid CLI mode: {{ mode }}" >&2
        echo "Expected one of: local, docker" >&2
        exit 2
        ;;
    esac

# Run a docs_rs_admin command using DOCSRS_CLI_MODE.
[group('cli')]
cli *args: _ensure_db_migrated
    SQLX_OFFLINE=1 just _run-cli "$DOCSRS_CLI_MODE" docs_rs_admin cli {{ args }}

_builder *args:
    # on non-linux (and arm/linux) we need to run the builder inside
    # a docker container because many places expect the builder to
    # run on a linux host.
    just _run-cli "$DOCSRS_BUILDER_CLI_MODE" docs_rs_builder builder-cli {{ args }}

# Run a docs_rs_builder command using DOCSRS_BUILDER_CLI_MODE.
[group('cli')]
builder *args: _ensure_db_migrated
    just _builder {{ args }}

# Run a docs_rs_watcher command using DOCSRS_CLI_MODE.
[group('cli')]
watcher *args: _ensure_db_migrated
    just _run-cli "$DOCSRS_CLI_MODE" docs_rs_watcher registry-watcher-cli {{ args }}

# Migrate to an optional version using docs_rs_admin and DOCSRS_CLI_MODE.
[group('cli')]
cli-db-migrate version="": compose-up-resources
    SQLX_OFFLINE=1 just _run-cli "$DOCSRS_CLI_MODE" docs_rs_admin cli database migrate {{ version }}

# Run the web server on the host.
[group('development')]
web *args: _ensure_db_migrated
    cargo run -p docs_rs_web -- {{ args }}

# Watch web and shared-library sources, restarting the host web server on changes.
[group('development')]
web-watch: _ensure_db_migrated (_ensure_cargo_installed "cargo-watch")
    cargo watch \
      --watch .cargo \
      --watch Cargo.lock \
      --watch Cargo.toml \
      --watch crates/bin/docs_rs_web \
      --watch crates/lib \
      --exec 'run -p docs_rs_web'

# Import a published crate release into the local environment.
[group('development')]
import-release +args: _ensure_db_migrated
    cargo run -p docs_rs_import_release -- {{ args }}
