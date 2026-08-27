
_ensure_db_and_s3_are_running: _touch-docker-env
    # dependencies in the docker-cli file are ignored
    # here. Instead we explicitly start any dependent services first.
    docker compose up -d db s3 elasticmq --wait

_touch-docker-env:
    touch .docker.env

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
