use docs_rs_opentelemetry::AnyMeterProvider;
use opentelemetry::{
    KeyValue,
    metrics::{Counter, Histogram},
};

#[derive(Debug)]
pub(crate) struct WatcherMetrics {
    pub(crate) sqs_messages_received_total: Counter<u64>,
    pub(crate) sqs_poll_errors_total: Counter<u64>,
    pub(crate) sqs_retries_total: Counter<u64>,
    pub(crate) changes_applied_total: Counter<u64>,
    pub(crate) sqs_message_processing_seconds: Histogram<f64>,
    pub(crate) sqs_event_lag_seconds: Histogram<f64>,
}

impl WatcherMetrics {
    pub(crate) fn new(meter_provider: &AnyMeterProvider) -> Self {
        let meter = meter_provider.meter("watcher");
        const PREFIX: &str = "docsrs.watcher";
        Self {
            sqs_messages_received_total: meter
                .u64_counter(format!("{PREFIX}.sqs_messages_received_total"))
                .with_unit("1")
                .build(),
            sqs_poll_errors_total: meter
                .u64_counter(format!("{PREFIX}.sqs_poll_errors_total"))
                .with_unit("1")
                .build(),
            sqs_retries_total: meter
                .u64_counter(format!("{PREFIX}.sqs_retries_total"))
                .with_unit("1")
                .build(),
            changes_applied_total: meter
                .u64_counter(format!("{PREFIX}.changes_applied_total"))
                .with_unit("1")
                .build(),
            sqs_message_processing_seconds: meter
                .f64_histogram(format!("{PREFIX}.sqs_message_processing_seconds"))
                .with_boundaries(vec![
                    0.001, 0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1.0, 2.5, 5.0, 10.0, 30.0,
                ])
                .with_unit("s")
                .build(),
            sqs_event_lag_seconds: meter
                .f64_histogram(format!("{PREFIX}.sqs_event_lag_seconds"))
                .with_boundaries(vec![
                    0.1, 0.5, 1.0, 5.0, 10.0, 30.0, 60.0, 300.0, 900.0, 3600.0,
                ])
                .with_unit("s")
                .build(),
        }
    }

    pub(crate) fn record_change_applied(&self, change_type: &'static str) {
        self.changes_applied_total
            .add(1, &[KeyValue::new("type", change_type)]);
    }
}
