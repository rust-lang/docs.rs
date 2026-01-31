use anyhow::{Error, Result};
use docs_rs_build_queue::AsyncBuildQueue;
use docs_rs_opentelemetry::AnyMeterProvider;
use opentelemetry::{KeyValue, metrics::Gauge};
use std::collections::HashSet;

#[derive(Debug)]
pub struct OtelServiceMetrics {
    pub queued_crates_count: Gauge<u64>,
    pub prioritized_crates_count: Gauge<u64>,
    pub queue_is_locked: Gauge<u64>,
    pub queued_crates_count_by_priority: Gauge<u64>,
}

impl OtelServiceMetrics {
    pub fn new(meter_provider: &AnyMeterProvider) -> Self {
        let meter = meter_provider.meter("service");
        const PREFIX: &str = "docsrs.service";
        Self {
            queued_crates_count: meter
                .u64_gauge(format!("{PREFIX}.queued_crates_count"))
                .with_unit("1")
                .build(),
            prioritized_crates_count: meter
                .u64_gauge(format!("{PREFIX}.prioritized_crates_count"))
                .with_unit("1")
                .build(),
            queue_is_locked: meter
                .u64_gauge(format!("{PREFIX}.queue_is_locked"))
                .with_unit("1")
                .build(),
            queued_crates_count_by_priority: meter
                .u64_gauge(format!("{PREFIX}.queued_crates_count_by_priority"))
                .with_unit("1")
                .build(),
        }
    }

    pub(crate) async fn gather(&self, queue: &AsyncBuildQueue) -> Result<(), Error> {
        self.queue_is_locked
            .record(queue.is_locked().await? as u64, &[]);
        self.queued_crates_count
            .record(queue.pending_count().await? as u64, &[]);
        self.prioritized_crates_count
            .record(queue.prioritized_count().await? as u64, &[]);

        let queue_pending_count = queue.pending_count_by_priority().await?;

        // gauges keep their old value per label when it's not removed, reset to zero or updated.
        // When a priority is used at least once, it would be kept in the metric and the last
        // value would be remembered. `pending_count_by_priority` returns only the priorities
        // that are currently in the queue, which means when the tasks for a priority are
        // finished, we wouldn't update the metric anymore, which means a wrong value is
        // in the metric.
        //
        // the only solution is to explicitly set the value to be zero, for all common priorities,
        // when there are no items in the queue with that priority.
        // So we create a set of all priorities we want to be explicitly zeroed, combined
        // with the actual priorities in the queue.
        let all_priorities: HashSet<i32> =
            queue_pending_count.keys().copied().chain(0..=20).collect();

        for priority in all_priorities {
            let count = queue_pending_count.get(&priority).unwrap_or(&0);

            self.queued_crates_count_by_priority.record(
                *count as u64,
                &[KeyValue::new("priority", priority.to_string())],
            );
        }

        Ok(())
    }
}
