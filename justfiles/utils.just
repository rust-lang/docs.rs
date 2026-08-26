_touch-docker-env:
    touch .docker.env
    touch .env

# Helper recipe to ensure a Cargo CLI tool is installed.
# The executable name defaults to the package name, but can be overridden for
# packages such as `sqlx-cli`, which installs an executable named `sqlx`.
# * uses `cargo binstall` if it exists.
#
# example usage:
# ```
# _ensure_sqlx_cli_installed: (_ensure_cargo_installed "sqlx-cli" "sqlx")
# ```
_ensure_cargo_installed package binary=package:
    #!/usr/bin/env bash
    set -euo pipefail

    if command -v "{{ binary }}" >/dev/null 2>&1; then
      exit 0
    fi

    if command -v cargo-binstall >/dev/null 2>&1; then
      cargo binstall -y "{{ package }}"
    else
      cargo install "{{ package }}"
    fi

_ensure_db_migrated:
    #!/usr/bin/env bash
    set -euo pipefail

    case "$DOCSRS_CLI_MODE" in
      local)
        just sqlx-migrate-run
        ;;
      docker)
        just cli-db-migrate
        ;;
      *)
        echo "Invalid CLI mode: $DOCSRS_CLI_MODE" >&2
        echo "Expected one of: local, docker" >&2
        exit 2
        ;;
    esac
