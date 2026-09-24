use docs_rs_crates_io::events::ChangeKind;
use docs_rs_opentelemetry::AnyMeterProvider;
use docs_rs_opentelemetry::{AnyMeterProvider, RESPONSE_TIME_HISTOGRAM_BUCKETS};
use opentelemetry::{
    KeyValue,
    metrics::{Counter, Histogram},
};
use std::{fmt, time::Duration};

/// Shared response-time buckets through 2 minutes, then doubling through 64 minutes.
const EVENT_PROCESSING_TIME_BUCKETS: &[Duration] = &{
    let mut buckets = [Duration::ZERO; RESPONSE_TIME_HISTOGRAM_BUCKETS.len() + 5];
    let mut i = 0;
    while i < RESPONSE_TIME_HISTOGRAM_BUCKETS.len() {
        buckets[i] = RESPONSE_TIME_HISTOGRAM_BUCKETS[i];
        i += 1;
    }
    while i < buckets.len() {
        buckets[i] = Duration::from_secs(buckets[i - 1].as_secs() * 2);
        i += 1;
    }
    buckets
};

#[derive(Debug, Clone, Copy)]
pub(crate) enum EventSource {
    Git,
    Sqs,
}

impl EventSource {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Git => "git",
            Self::Sqs => "sqs",
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
    events_received_total: Counter<u64>,
    poll_errors_total: Counter<u64>,
    changes_applied_total: Counter<u64>,
    event_processing_time: Histogram<f64>,
    event_lag: Histogram<f64>,
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
                .with_boundaries(
                    EVENT_PROCESSING_TIME_BUCKETS
                        .iter()
                        .map(|duration| duration.as_secs_f64())
                        .collect(),
                )
                .with_unit("s")
                .build(),
            event_lag: meter
                .f64_histogram(format!("{PREFIX}.event_lag"))
                .with_boundaries(vec![
                    0.1, 0.5, 1.0, 5.0, 10.0, 30.0, 60.0, 300.0, 900.0, 3600.0,
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

    pub(crate) fn record_event_lag(&self, source: EventSource, duration: Duration) {
        self.event_lag.record(
            duration.as_secs_f64(),
            &[KeyValue::new("source", source.as_str())],
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
