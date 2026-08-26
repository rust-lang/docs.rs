# Testing Metrics

Unit tests can inspect emitted metrics using the test metric provider from
`docs_rs_opentelemetry`; tests using the shared context can call
`TestEnvironment::collected_metrics`. For an end-to-end check, the Docker
Compose setup provides an OpenTelemetry Collector configured to log the metrics
it receives.

Start the collector with:

```console
$ just compose-up-metrics
```

Configure the docs.rs process to export OTLP metrics to it:

```dotenv
OTEL_EXPORTER_OTLP_ENDPOINT=http://localhost:4317
```

Put this setting in `.env` when running docs.rs on the host, or in `.docker.env`
when running it through Docker Compose. Restart the application after changing
its environment.

The collector uses OTLP over gRPC on port 4317 and writes received metrics to
its logs. Follow them with:

```console
$ docker compose logs --follow opentelemetry
```

If `OTEL_EXPORTER_OTLP_ENDPOINT` is unset, docs.rs uses a no-op metric provider.
