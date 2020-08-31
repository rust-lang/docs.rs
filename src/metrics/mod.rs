#[macro_use]
mod macros;

use self::macros::MetricFromOpts;
use crate::db::Pool;
use crate::BuildQueue;
use dashmap::DashMap;
use failure::Error;
use prometheus::proto::MetricFamily;
use std::time::{Duration, Instant};

load_metric_type!(IntGauge as single);
load_metric_type!(IntCounter as single);
load_metric_type!(IntCounterVec as vec);
load_metric_type!(IntGaugeVec as vec);
load_metric_type!(HistogramVec as vec);

metrics! {
    pub struct Metrics {
        /// Number of crates in the build queue
        queued_crates_count: IntGauge,
        /// Number of crates in the build queue that have a positive priority
        prioritized_crates_count: IntGauge,
        /// Number of crates that failed to build
        failed_crates_count: IntGauge,

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
        /// The time it takes to render a rustdoc page
        pub(crate) rustdoc_rendering_times: HistogramVec["step"],

        /// Count of recently accessed crates
        pub(crate) recent_crates: IntGaugeVec["duration"],
        /// Count of recently accessed versions of crates
        pub(crate) recent_versions: IntGaugeVec["duration"],
        /// Count of recently accessed platforms of versions of crates
        pub(crate) recent_platforms: IntGaugeVec["duration"],

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
    }

    // The Rust prometheus library treats the namespace as the "prefix" of the metric name: a
    // metric named `foo` with a prefix of `docsrs` will expose a metric called `docsrs_foo`.
    //
    // https://docs.rs/prometheus/0.9.0/prometheus/struct.Opts.html#structfield.namespace
    namespace: "docsrs",
}

#[derive(Debug, Default)]
pub(crate) struct RecentReleases {
    krates: DashMap<String, Instant>,
    versions: DashMap<String, Instant>,
    platforms: DashMap<String, Instant>,
}

impl RecentReleases {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    pub(crate) fn record(&self, krate: &str, version: &str, target: &str) {
        self.krates.insert(krate.to_owned(), Instant::now());
        self.versions
            .insert(format!("{}/{}", krate, version), Instant::now());
        self.platforms
            .insert(format!("{}/{}/{}", krate, version, target), Instant::now());
    }

    pub(crate) fn gather(&self, metrics: &Metrics) {
        fn inner(map: &DashMap<String, Instant>, metric: &IntGaugeVec) {
            let mut hour_count = 0;
            let mut half_hour_count = 0;
            let mut five_minute_count = 0;
            map.retain(|_, instant| {
                let elapsed = instant.elapsed();
                if elapsed > Duration::from_secs(60 * 60) {
                    return false;
                }
                hour_count += 1;
                if elapsed > Duration::from_secs(30 * 60) {
                    return true;
                }
                half_hour_count += 1;
                if elapsed > Duration::from_secs(5 * 60) {
                    return true;
                }
                five_minute_count += 1;
                true
            });

            metric.with_label_values(&["one hour"]).set(hour_count);

            metric
                .with_label_values(&["half hour"])
                .set(half_hour_count);

            metric
                .with_label_values(&["five minutes"])
                .set(five_minute_count);
        }

        inner(&self.krates, &metrics.recent_crates);
        inner(&self.versions, &metrics.recent_versions);
        inner(&self.platforms, &metrics.recent_platforms);
    }
}

impl Metrics {
    pub(crate) fn gather(
        &self,
        pool: &Pool,
        queue: &BuildQueue,
    ) -> Result<Vec<MetricFamily>, Error> {
        self.idle_db_connections.set(pool.idle_connections() as i64);
        self.used_db_connections.set(pool.used_connections() as i64);
        self.max_db_connections.set(pool.max_size() as i64);

        self.queued_crates_count.set(queue.pending_count()? as i64);
        self.prioritized_crates_count
            .set(queue.prioritized_count()? as i64);
        self.failed_crates_count.set(queue.failed_count()? as i64);

        self.recent_releases.gather(self);
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
            .set(process.fd().unwrap().len() as i64);
        self.running_threads
            .set(process.stat().unwrap().num_threads as i64);
    }
}
