
_ensure_db_and_s3_are_running: _touch-docker-env
    # dependencies in the docker-cli file are ignored
    # here. Instead we explicitly start any dependent services first.
    docker compose up -d db s3 --wait

_touch-docker-env:
    touch .docker.env

send-sqs-payload:
    aws sqs send-message \
      --endpoint-url $DOCSRS_SQS_ENDPOINT_URL \
      --region elasticmq \
      --queue-url $DOCSRS_SQS_QUEUE_URL \
      --message-body '{"id":"evt_1","occurred_at":"2026-07-02T12:00:00Z","type":"added","payload":{"name":"demo-crate","vers":"1.2.3"}}'
