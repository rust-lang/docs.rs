use docs_rs_crates_io::events::ChangeKind;
use docs_rs_opentelemetry::AnyMeterProvider;
use opentelemetry::{
    KeyValue,
    metrics::{Counter, Histogram},
};
use std::{fmt, time::Duration};

#[derive(Debug, Clone, Copy)]
pub(crate) enum EventSource {
    Git,
    // NOTE: Sqs will be added later
}

impl EventSource {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Git => "git",
        }
    }
}

impl fmt::Display for EventSource {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug)]
pub(crate) struct WatcherMetrics {
    /// received event count, by source
    events_received_total: Counter<u64>,
    /// poll errors, by source
    poll_errors_total: Counter<u64>,
    /// changes applied, by source and change-kind
    changes_applied_total: Counter<u64>,
    /// event processing time, by source and change-kind
    event_processing_time: Histogram<f64>,
}

impl WatcherMetrics {
    pub(crate) fn new(meter_provider: &AnyMeterProvider) -> Self {
        let meter = meter_provider.meter("watcher");
        const PREFIX: &str = "docsrs.watcher";
        Self {
            events_received_total: meter
                .u64_counter(format!("{PREFIX}.events_received_total"))
                .with_unit("1")
                .build(),
            poll_errors_total: meter
                .u64_counter(format!("{PREFIX}.poll_errors_total"))
                .with_unit("1")
                .build(),
            changes_applied_total: meter
                .u64_counter(format!("{PREFIX}.changes_applied_total"))
                .with_unit("1")
                .build(),
            event_processing_time: meter
                .f64_histogram(format!("{PREFIX}.event_processing_time"))
                // Boundaries for the histogram, should be min/max for the processing time
                .with_boundaries(vec![
                    // that's what we expect in processing time, between <1s, and 5 minutes.
                    0.001, 0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1.0, 2.5, 5.0, 10.0, 30.0,
                    45.0, 55.0, 60.0, 65.0, 90.0, 120.0, 180.0, 300.0,
                    // these are just for outliers, so we see them
                    600.0, 900.0, 1800.0, 3600.0,
                ])
                .with_unit("s")
                .build(),
        }
    }

    pub(crate) fn record_change_applied(&self, source: EventSource, kind: ChangeKind) {
        self.changes_applied_total.add(
            1,
            &[
                KeyValue::new("source", source.as_str()),
                KeyValue::new("type", kind.as_str()),
            ],
        );
    }

    pub(crate) fn record_event_processing_time(
        &self,
        source: EventSource,
        kind: Option<ChangeKind>,
        success: bool,
        duration: Duration,
    ) {
        let result = if success { "ok" } else { "err" };
        self.event_processing_time.record(
            duration.as_secs_f64(),
            &[
                KeyValue::new("source", source.as_str()),
                KeyValue::new("type", kind.map(ChangeKind::as_str).unwrap_or("unknown")),
                KeyValue::new("result", result),
            ],
        );
    }

    pub(crate) fn record_events_received(&self, source: EventSource, count: usize) {
        self.events_received_total
            .add(count as u64, &[KeyValue::new("source", source.as_str())]);
    }

    pub(crate) fn record_poll_error(&self, source: EventSource) {
        self.poll_errors_total
            .add(1, &[KeyValue::new("source", source.as_str())]);
    }
}
