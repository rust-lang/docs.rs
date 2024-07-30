#[macro_use]
mod macros;

use self::macros::MetricFromOpts;
use crate::{cdn, db::Pool, target::TargetAtom, BuildQueue, Config};
use anyhow::Error;
use dashmap::DashMap;
use prometheus::proto::MetricFamily;
use std::{
    collections::HashSet,
    time::{Duration, Instant},
};

load_metric_type!(IntGauge as single);
load_metric_type!(IntCounter as single);
load_metric_type!(IntCounterVec as vec);
load_metric_type!(IntGaugeVec as vec);
load_metric_type!(HistogramVec as vec);

/// the measured times from cdn invalidations, meaning:
/// * how long an invalidation took, or
/// * how long the invalidation was queued
///
/// will be put into these buckets (seconds,
/// each entry is the upper bound).
/// Prometheus only gets the counts per bucket in a certain
/// time range, no exact durations.
pub const CDN_INVALIDATION_HISTOGRAM_BUCKETS: &[f64; 11] = &[
    60.0,    // 1
    120.0,   // 2
    300.0,   // 5
    600.0,   // 10
    900.0,   // 15
    1200.0,  // 20
    1800.0,  // 30
    2700.0,  // 45
    6000.0,  // 100
    12000.0, // 200
    24000.0, // 400
];

/// the measured times of building crates will be put into these buckets
pub fn build_time_histogram_buckets() -> Vec<f64> {
    vec![
        30.0,   // 0.5
        60.0,   // 1
        120.0,  // 2
        180.0,  // 3
        240.0,  // 4
        300.0,  // 5
        360.0,  // 6
        420.0,  // 7
        480.0,  // 8
        540.0,  // 9
        600.0,  // 10
        660.0,  // 11
        720.0,  // 12
        780.0,  // 13
        840.0,  // 14
        900.0,  // 15
        1200.0, // 20
        1800.0, // 30
        2400.0, // 40
        3000.0, // 50
        3600.0, // 60
    ]
}

metrics! {
    pub struct InstanceMetrics {
        /// The number of idle database connections
        idle_db_connections: IntGauge,
        /// The number of used database connections
        used_db_connections: IntGauge,
        /// The maximum number of database connections
        max_db_connections: IntGauge,
        /// Number of attempted and failed connections to the database
        pub(crate) failed_db_connections: IntCounter,

        /// The number of currently opened file descriptors
        #[cfg(target_os = "linux")]
        open_file_descriptors: IntGauge,
        /// The number of threads being used by docs.rs
        #[cfg(target_os = "linux")]
        running_threads: IntGauge,

        /// The traffic of various docs.rs routes
        pub(crate) routes_visited: IntCounterVec["route"],
        /// The response times of various docs.rs routes
        pub(crate) response_time: HistogramVec["route"],

        /// Count of recently accessed crates
        pub(crate) recent_crates: IntGaugeVec["duration"],
        /// Count of recently accessed versions of crates
        pub(crate) recent_versions: IntGaugeVec["duration"],
        /// Count of recently accessed platforms of versions of crates
        pub(crate) recent_platforms: IntGaugeVec["duration"],

        /// number of queued builds
        pub(crate) queued_builds: IntCounter,
        /// Number of crates built
        pub(crate) total_builds: IntCounter,
        /// Number of builds that successfully generated docs
        pub(crate) successful_builds: IntCounter,
        /// Number of builds that generated a compiler error
        pub(crate) failed_builds: IntCounter,
        /// Number of builds that did not complete due to not being a library
        pub(crate) non_library_builds: IntCounter,

        /// Number of files uploaded to the storage backend
        pub(crate) uploaded_files_total: IntCounter,

        /// The number of attempted files that failed due to a memory limit
        pub(crate) html_rewrite_ooms: IntCounter,

        /// the number of "I'm feeling lucky" searches for crates
        pub(crate) im_feeling_lucky_searches: IntCounter,
    }

    // The Rust prometheus library treats the namespace as the "prefix" of the metric name: a
    // metric named `foo` with a prefix of `docsrs` will expose a metric called `docsrs_foo`.
    //
    // https://docs.rs/prometheus/0.9.0/prometheus/struct.Opts.html#structfield.namespace
    namespace: "docsrs",
}

/// Converts a `Duration` to seconds, used by prometheus internally
#[inline]
pub(crate) fn duration_to_seconds(d: Duration) -> f64 {
    let nanos = f64::from(d.subsec_nanos()) / 1e9;
    d.as_secs() as f64 + nanos
}

#[derive(Debug, Default)]
pub(crate) struct RecentlyAccessedReleases {
    crates: DashMap<i32, Instant>,
    versions: DashMap<i32, Instant>,
    platforms: DashMap<(i32, TargetAtom), Instant>,
}

impl RecentlyAccessedReleases {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    pub(crate) fn record(&self, krate: i32, version: i32, target: &str) {
        if self.platforms.len() > 100_000 {
            // Avoid filling the maps _too_ much, we should never get anywhere near this limit
            return;
        }

        let now = Instant::now();
        self.crates.insert(krate, now);
        self.versions.insert(version, now);
        self.platforms
            .insert((version, TargetAtom::from(target)), now);
    }

    pub(crate) fn gather(&self, metrics: &InstanceMetrics) {
        fn inner<K: std::hash::Hash + Eq>(map: &DashMap<K, Instant>, metric: &IntGaugeVec) {
            let mut hour_count = 0;
            let mut half_hour_count = 0;
            let mut five_minute_count = 0;
            map.retain(|_, instant| {
                let elapsed = instant.elapsed();

                if elapsed < Duration::from_secs(60 * 60) {
                    hour_count += 1;
                }
                if elapsed < Duration::from_secs(30 * 60) {
                    half_hour_count += 1;
                }
                if elapsed < Duration::from_secs(5 * 60) {
                    five_minute_count += 1;
                }

                // Only retain items accessed within the last hour
                elapsed < Duration::from_secs(60 * 60)
            });

            metric.with_label_values(&["one hour"]).set(hour_count);

            metric
                .with_label_values(&["half hour"])
                .set(half_hour_count);

            metric
                .with_label_values(&["five minutes"])
                .set(five_minute_count);
        }

        inner(&self.crates, &metrics.recent_crates);
        inner(&self.versions, &metrics.recent_versions);
        inner(&self.platforms, &metrics.recent_platforms);
    }
}

impl InstanceMetrics {
    pub(crate) fn gather(&self, pool: &Pool) -> Result<Vec<MetricFamily>, Error> {
        self.idle_db_connections.set(pool.idle_connections() as i64);
        self.used_db_connections.set(pool.used_connections() as i64);
        self.max_db_connections.set(pool.max_size() as i64);

        self.recently_accessed_releases.gather(self);
        self.gather_system_performance();
        Ok(self.registry.gather())
    }

    #[cfg(not(target_os = "linux"))]
    fn gather_system_performance(&self) {}

    #[cfg(target_os = "linux")]
    fn gather_system_performance(&self) {
        use procfs::process::Process;

        let process = Process::myself().unwrap();
        self.open_file_descriptors
            .set(process.fd_count().unwrap() as i64);
        self.running_threads
            .set(process.stat().unwrap().num_threads);
    }
}

fn metric_from_opts<T: MetricFromOpts + Clone + prometheus::core::Collector + 'static>(
    registry: &prometheus::Registry,
    metric: &str,
    help: &str,
    variable_label: Option<&str>,
) -> Result<T, prometheus::Error> {
    let mut opts = prometheus::Opts::new(metric, help).namespace("docsrs");

    if let Some(label) = variable_label {
        opts = opts.variable_label(label);
    }

    let metric = T::from_opts(opts)?;
    registry.register(Box::new(metric.clone()))?;
    Ok(metric)
}

#[derive(Debug)]
pub struct ServiceMetrics {
    pub queued_crates_count: IntGauge,
    pub prioritized_crates_count: IntGauge,
    pub failed_crates_count: IntGauge,
    pub queue_is_locked: IntGauge,
    pub queued_crates_count_by_priority: IntGaugeVec,
    pub queued_cdn_invalidations_by_distribution: IntGaugeVec,

    registry: prometheus::Registry,
}

impl ServiceMetrics {
    pub fn new() -> Result<Self, prometheus::Error> {
        let registry = prometheus::Registry::new();
        Ok(Self {
            registry: registry.clone(),
            queued_crates_count: metric_from_opts(
                &registry,
                "queued_crates_count",
                "Number of crates in the build queue",
                None,
            )?,
            prioritized_crates_count: metric_from_opts(
                &registry,
                "prioritized_crates_count",
                "Number of crates in the build queue that have a positive priority",
                None,
            )?,
            failed_crates_count: metric_from_opts(
                &registry,
                "failed_crates_count",
                "Number of crates that failed to build",
                None,
            )?,
            queue_is_locked: metric_from_opts(
                &registry,
                "queue_is_locked",
                "Whether the build queue is locked",
                None,
            )?,
            queued_crates_count_by_priority: metric_from_opts(
                &registry,
                "queued_crates_count_by_priority",
                "queued crates by priority",
                Some("priority"),
            )?,
            queued_cdn_invalidations_by_distribution: metric_from_opts(
                &registry,
                "queued_cdn_invalidations_by_distribution",
                "queued CDN invalidations",
                Some("distribution"),
            )?,
        })
    }

    pub(crate) fn gather(
        &self,
        pool: &Pool,
        queue: &BuildQueue,
        config: &Config,
    ) -> Result<Vec<MetricFamily>, Error> {
        self.queue_is_locked.set(queue.is_locked()? as i64);
        self.queued_crates_count.set(queue.pending_count()? as i64);
        self.prioritized_crates_count
            .set(queue.prioritized_count()? as i64);

        let queue_pending_count = queue.pending_count_by_priority()?;

        // gauges keep their old value per label when it's not removed, reset to zero or updated.
        // When a priority is used at least once, it would be kept in the metric and the last
        // value would be remembered. `pending_count_by_priority` returns only the priorities
        // that are currently in the queue, which means when the tasks for a priority are
        // finished, we wouldn't update the metric any more, which means a wrong value is
        // in the metric.
        //
        // The solution is to reset the metric, and then set all priorities again.
        self.queued_crates_count_by_priority.reset();

        // for commonly used priorities we want the value to be zero, and not missing,
        // when there are no items in the queue with that priority.
        // So we create a set of all priorities we want to be explicitly zeroed, combined
        // with the actual priorities in the queue.
        let all_priorities: HashSet<i32> =
            queue_pending_count.keys().copied().chain(0..=20).collect();

        for priority in all_priorities {
            let count = queue_pending_count.get(&priority).unwrap_or(&0);

            self.queued_crates_count_by_priority
                .with_label_values(&[&priority.to_string()])
                .set(*count as i64);
        }

        let mut conn = pool.get()?;
        for (distribution_id, count) in
            cdn::queued_or_active_crate_invalidation_count_by_distribution(&mut *conn, config)?
        {
            self.queued_cdn_invalidations_by_distribution
                .with_label_values(&[&distribution_id])
                .set(count);
        }

        self.failed_crates_count.set(queue.failed_count()? as i64);
        Ok(self.registry.gather())
    }
}
