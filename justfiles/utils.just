_touch-docker-env:
    touch .docker.env
    touch .env

send-sqs-payload:
    #!/usr/bin/env bash
    set -euo pipefail

    payload='{
      "id": "evt_1",
      "occurred_at": "2026-07-02T12:00:00Z",
      "type": "added",
      "payload": {
        "name": "demo-crate",
        "vers": "1.2.3"
      }
    }'

    aws sqs send-message \
      --endpoint-url $DOCSRS_SQS_ENDPOINT_URL \
      --region elasticmq \
      --queue-url $DOCSRS_SQS_QUEUE_URL \
      --message-body "$payload"

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
