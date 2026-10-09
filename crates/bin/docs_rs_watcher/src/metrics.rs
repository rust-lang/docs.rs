use docs_rs_crates_io::events::ChangeKind;
use docs_rs_opentelemetry::{AnyMeterProvider, RESPONSE_TIME_HISTOGRAM_BUCKETS};
use docs_rs_types::Duration;
use opentelemetry::{
    KeyValue,
    metrics::{Counter, Histogram},
};
use std::{fmt, time::Duration as StdDuration};

/// Publication-to-enqueue times from one second through one week.
const RELEASE_ENQUEUE_LATENCY_BUCKETS: &[Duration; 16] = &[
    Duration::from_secs(1),
    Duration::from_secs(5),
    Duration::from_secs(10),
    Duration::from_secs(30),
    Duration::from_mins(1),
    Duration::from_mins(2),
    Duration::from_mins(5),
    Duration::from_mins(10),
    Duration::from_mins(30),
    Duration::from_hours(1),
    Duration::from_hours(2),
    Duration::from_hours(6),
    Duration::from_hours(12),
    Duration::from_days(1),
    Duration::from_days(2),
    Duration::from_weeks(1),
];

/// Shared response-time buckets through 2 minutes, then doubling through 64 minutes.
const EVENT_PROCESSING_TIME_BUCKETS: &[StdDuration] = &{
    let mut buckets = [StdDuration::ZERO; RESPONSE_TIME_HISTOGRAM_BUCKETS.len() + 5];
    let mut i = 0;
    while i < RESPONSE_TIME_HISTOGRAM_BUCKETS.len() {
        buckets[i] = RESPONSE_TIME_HISTOGRAM_BUCKETS[i];
        i += 1;
    }
    while i < buckets.len() {
        buckets[i] = StdDuration::from_secs(buckets[i - 1].as_secs() * 2);
        i += 1;
    }
    buckets
};

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
    release_enqueue_latency: Histogram<f64>,
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
            release_enqueue_latency: meter
                .f64_histogram(format!("{PREFIX}.release_enqueue_latency"))
                .with_unit("s")
                .with_boundaries(
                    RELEASE_ENQUEUE_LATENCY_BUCKETS
                        .iter()
                        .map(|duration| duration.as_secs_f64())
                        .collect(),
                )
                .with_description("Time from registry publication to successful enqueue")
                .build(),
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
        }
    }

    pub(crate) fn record_release_enqueue_latency(&self, elapsed: Duration) {
        self.release_enqueue_latency
            .record(elapsed.as_secs_f64(), &[]);
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
