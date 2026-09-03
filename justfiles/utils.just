_touch-docker-env:
    touch .docker.env
    touch .env

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
