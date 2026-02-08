use crate::{AsyncBuildQueue, QueuedCrate, types::BuildPackageSummary};
use anyhow::Result;
use docs_rs_types::{KrateName, Version};
use docs_rs_utils::Handle;
use sqlx::Connection as _;
#[cfg(test)]
use std::collections::HashMap;
use std::sync::Arc;
use tokio::runtime;
use tracing::error;

#[derive(Debug)]
pub struct BuildQueue {
    runtime: Handle,
    inner: Arc<AsyncBuildQueue>,
}

/// sync versions of async methods
impl BuildQueue {
    pub fn add_crate(
        &self,
        name: &KrateName,
        version: &Version,
        priority: i32,
        registry: Option<&str>,
    ) -> Result<()> {
        self.runtime
            .block_on(self.inner.add_crate(name, version, priority, registry))
    }

    pub fn is_locked(&self) -> Result<bool> {
        self.runtime.block_on(self.inner.is_locked())
    }
    pub fn lock(&self) -> Result<()> {
        self.runtime.block_on(self.inner.lock())
    }
    pub fn unlock(&self) -> Result<()> {
        self.runtime.block_on(self.inner.unlock())
    }

    #[cfg(test)]
    pub(crate) fn pending_count(&self) -> Result<usize> {
        self.runtime.block_on(self.inner.pending_count())
    }
    #[cfg(test)]
    pub(crate) fn prioritized_count(&self) -> Result<usize> {
        self.runtime.block_on(self.inner.prioritized_count())
    }
    #[cfg(test)]
    pub(crate) fn pending_count_by_priority(&self) -> Result<HashMap<i32, usize>> {
        self.runtime
            .block_on(self.inner.pending_count_by_priority())
    }
    #[cfg(test)]
    pub(crate) fn queued_crates(&self) -> Result<Vec<QueuedCrate>> {
        self.runtime.block_on(self.inner.queued_crates())
    }
}

impl BuildQueue {
    pub fn new(runtime: runtime::Handle, inner: Arc<AsyncBuildQueue>) -> Self {
        Self {
            runtime: runtime.into(),
            inner,
        }
    }

    pub fn process_next_crate(
        &self,
        f: impl FnOnce(&QueuedCrate) -> Result<BuildPackageSummary>,
    ) -> Result<Option<i32>> {
        let mut conn = self.runtime.block_on(self.inner.db.get_async())?;
        let mut transaction = self.runtime.block_on(conn.begin())?;

        // fetch the next available crate from the queue table.
        // We are using `SELECT FOR UPDATE` inside a transaction so
        // the QueuedCrate is locked until we are finished with it.
        // `SKIP LOCKED` here will enable another build-server to just
        // skip over taken (=locked) rows and start building the first
        // available one.
        let to_process = match self.runtime.block_on(
            sqlx::query_as!(
                QueuedCrate,
                r#"SELECT
                    id,
                    name as "name: KrateName",
                    version as "version: Version",
                    priority,
                    registry,
                    attempt
                 FROM queue
                 WHERE
                    last_attempt IS NULL OR last_attempt < NOW() - make_interval(secs => $1)
                 ORDER BY priority ASC, attempt ASC, id ASC
                 LIMIT 1
                 FOR UPDATE SKIP LOCKED"#,
                self.inner.config.delay_between_build_attempts.as_secs_f64(),
            )
            .fetch_optional(&mut *transaction),
        )? {
            Some(krate) => krate,
            None => return Ok(None),
        };

        let res = f(&to_process);

        let delete_from_queue = |conn: &mut sqlx::PgConnection| -> Result<()> {
            self.runtime.block_on(
                sqlx::query!("DELETE FROM queue WHERE id = $1;", to_process.id).execute(conn),
            )?;
            Ok(())
        };

        let increase_attempt_or_delete_from_queue =
            |conn: &mut sqlx::PgConnection| -> Result<Option<i32>> {
                let potential_next_attempt = self.runtime.block_on(
                    sqlx::query_scalar!(
                        "UPDATE queue
                         SET
                            attempt = attempt + 1,
                            last_attempt = NOW()
                         WHERE id = $1
                         RETURNING attempt;",
                        to_process.id,
                    )
                    .fetch_one(&mut *conn),
                )?;

                if potential_next_attempt >= self.inner.config.build_attempts.into() {
                    self.inner.queue_metrics.failed_crates_count.add(1, &[]);
                    // exceeded max attempts, remove from queue
                    delete_from_queue(&mut *conn)?;
                    Ok(None)
                } else {
                    // keep in queue for re-attempt
                    Ok(Some(potential_next_attempt))
                }
            };

        let next_attempt: Option<i32>;

        match res {
            Ok(BuildPackageSummary {
                should_reattempt: false,
                successful: _,
            }) => {
                delete_from_queue(&mut transaction)?;
                next_attempt = None;
            }
            Ok(BuildPackageSummary {
                should_reattempt: true,
                successful: _,
            }) => {
                next_attempt = increase_attempt_or_delete_from_queue(&mut transaction)?;
            }
            Err(e) => {
                next_attempt = increase_attempt_or_delete_from_queue(&mut transaction)?;

                error!(
                    ?e,
                    name = %to_process.name,
                    version = %to_process.version,
                    "Failed to build package"
                );
            }
        }

        self.runtime.block_on(transaction.commit())?;
        Ok(next_attempt)
    }
}

#[cfg(test)]
mod tests {
    use crate::Config;

    use super::*;
    use chrono::Utc;
    use docs_rs_config::AppConfig as _;
    use docs_rs_database::{AsyncPoolClient, testing::TestDatabase};
    use docs_rs_opentelemetry::testing::TestMetrics;
    use docs_rs_types::testing::{KRATE, V1, V2};
    use docs_rs_utils::block_on_async_with_conn;
    use pretty_assertions::assert_eq;
    use std::time::Duration;

    const FOO: KrateName = KrateName::from_static("foo");
    const BAR: KrateName = KrateName::from_static("bar");
    const BAZ: KrateName = KrateName::from_static("baz");

    // when we start migrating / spitting the binaries,
    // we probably will create  amore powerfull & flexible
    // test& app context. Then we could migrate this.
    struct TestEnv {
        db: TestDatabase,
        queue: BuildQueue,
        metrics: TestMetrics,
        runtime: runtime::Runtime,
    }

    impl TestEnv {
        pub(crate) fn runtime(&self) -> &runtime::Runtime {
            &self.runtime
        }

        pub async fn async_conn(&self) -> Result<AsyncPoolClient> {
            self.db.async_conn().await
        }

        fn queued_builds(&self) -> Result<u64> {
            let collected_metrics = self.metrics.collected_metrics();

            Ok(collected_metrics
                .get_metric("build_queue", "docsrs.build_queue.queued_builds")?
                .get_u64_counter()
                .value())
        }

        fn failed_count(&self) -> u64 {
            let collected_metrics = self.metrics.collected_metrics();

            if let Ok(metric) = collected_metrics
                .get_metric("build_queue", "docsrs.build_queue.failed_crates_count")
            {
                metric.get_u64_counter().value()
            } else {
                0
            }
        }
    }

    fn test_queue(config: Config) -> Result<TestEnv> {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()?;

        let metrics = TestMetrics::new();
        let db = runtime.block_on(TestDatabase::new(
            &docs_rs_database::Config::test_config()?,
            metrics.provider(),
        ))?;

        let async_queue = Arc::new(AsyncBuildQueue::new(
            db.pool().clone(),
            Arc::new(config),
            metrics.provider(),
        ));

        Ok(TestEnv {
            db,
            queue: BuildQueue {
                runtime: runtime.handle().clone().into(),
                inner: async_queue,
            },
            metrics,
            runtime,
        })
    }

    #[test]
    fn test_wait_between_build_attempts() -> Result<()> {
        let env = test_queue(Config {
            build_attempts: 99,
            delay_between_build_attempts: Duration::from_secs(1),
            ..Default::default()
        })?;

        let queue = &env.queue;

        queue.add_crate(&KRATE, &V1, 0, None)?;

        // first let it fail
        queue.process_next_crate(|krate| {
            assert_eq!(krate.name, KRATE);
            anyhow::bail!("simulate a failure");
        })?;

        queue.process_next_crate(|_| {
            // this can't happen since we didn't wait between attempts
            unreachable!();
        })?;

        block_on_async_with_conn!(env, |mut conn| async {
            // fake the build-attempt timestamp so it's older
            Ok(sqlx::query!(
                "UPDATE queue SET last_attempt = $1",
                Utc::now() - chrono::Duration::try_seconds(60).unwrap()
            )
            .execute(&mut *conn)
            .await?)
        })?;

        let mut handled = false;
        // now we can process it again
        queue.process_next_crate(|krate| {
            assert_eq!(krate.name, KRATE);
            handled = true;
            Ok(BuildPackageSummary::default())
        })?;

        assert!(handled);

        Ok(())
    }

    #[test]
    fn test_add_and_process_crates() -> Result<()> {
        const MAX_ATTEMPTS: u16 = 3;
        let env = test_queue(Config {
            build_attempts: MAX_ATTEMPTS,
            delay_between_build_attempts: Duration::ZERO,
            ..Default::default()
        })?;
        let queue = &env.queue;

        const LOW_PRIORITY: KrateName = KrateName::from_static("low-priority");
        const HIGH_PRIORITY_FOO: KrateName = KrateName::from_static("high-priority-foo");
        const MEDIUM_PRIORITY: KrateName = KrateName::from_static("medium-priority");
        const HIGH_PRIORITY_BAR: KrateName = KrateName::from_static("high-priority-bar");
        const STANDARD_PRIORITY: KrateName = KrateName::from_static("standard-priority");
        const HIGH_PRIORITY_BAZ: KrateName = KrateName::from_static("high-priority-baz");

        let test_crates = [
            (LOW_PRIORITY, 1000),
            (HIGH_PRIORITY_FOO, -1000),
            (MEDIUM_PRIORITY, -10),
            (HIGH_PRIORITY_BAR, -1000),
            (STANDARD_PRIORITY, 0),
            (HIGH_PRIORITY_BAZ, -1000),
        ];
        for krate in &test_crates {
            queue.add_crate(&krate.0, &V1, krate.1, None)?;
        }

        let assert_next = |name| -> Result<()> {
            queue.process_next_crate(|krate| {
                assert_eq!(name, krate.name);
                Ok(BuildPackageSummary::default())
            })?;
            Ok(())
        };
        let assert_next_and_fail = |name| -> Result<()> {
            queue.process_next_crate(|krate| {
                assert_eq!(name, krate.name);
                anyhow::bail!("simulate a failure");
            })?;
            Ok(())
        };

        // The first processed item is the one with the highest priority added first.
        assert_next(HIGH_PRIORITY_FOO)?;

        // Simulate a failure in high-priority-bar.
        assert_next_and_fail(HIGH_PRIORITY_BAR)?;

        // Continue with the next high priority crate.
        assert_next(HIGH_PRIORITY_BAZ)?;

        // After all the crates with the max priority are processed, before starting to process
        // crates with a lower priority the failed crates with the max priority will be tried
        // again.
        assert_next(HIGH_PRIORITY_BAR)?;

        // Continue processing according to the priority.
        assert_next(MEDIUM_PRIORITY)?;
        assert_next(STANDARD_PRIORITY)?;

        // Simulate the crate failing many times.
        for _ in 0..MAX_ATTEMPTS {
            assert_next_and_fail(LOW_PRIORITY)?;
        }

        // Since low-priority failed many times it will be removed from the queue. Because of
        // that the queue should now be empty.
        let mut called = false;
        queue.process_next_crate(|_| {
            called = true;
            Ok(BuildPackageSummary::default())
        })?;
        assert!(!called, "there were still items in the queue");

        assert_eq!(env.queued_builds()?, test_crates.len() as u64);

        Ok(())
    }

    #[test]
    fn test_pending_count() -> Result<()> {
        let env = test_queue(Config::default())?;
        let queue = env.queue;
        assert_eq!(queue.pending_count()?, 0);
        queue.add_crate(&FOO, &V1, 0, None)?;
        assert_eq!(queue.pending_count()?, 1);
        queue.add_crate(&BAR, &V1, 0, None)?;
        assert_eq!(queue.pending_count()?, 2);

        queue.process_next_crate(|krate| {
            assert_eq!(FOO, krate.name);
            Ok(BuildPackageSummary::default())
        })?;
        assert_eq!(queue.pending_count()?, 1);

        Ok(())
    }

    #[test]
    fn test_prioritized_count() -> Result<()> {
        let env = test_queue(Config::default())?;
        let queue = env.queue;

        assert_eq!(queue.prioritized_count()?, 0);
        queue.add_crate(&FOO, &V1, 0, None)?;
        assert_eq!(queue.prioritized_count()?, 1);
        queue.add_crate(&BAR, &V1, -100, None)?;
        assert_eq!(queue.prioritized_count()?, 2);
        queue.add_crate(&BAZ, &V1, 100, None)?;
        assert_eq!(queue.prioritized_count()?, 2);

        queue.process_next_crate(|krate| {
            assert_eq!(BAR, krate.name);
            Ok(BuildPackageSummary::default())
        })?;
        assert_eq!(queue.prioritized_count()?, 1);

        Ok(())
    }

    #[test]
    fn test_count_by_priority() -> Result<()> {
        let env = test_queue(Config::default())?;
        let queue = env.queue;

        assert!(queue.pending_count_by_priority()?.is_empty());

        queue.add_crate(&FOO, &V1, 1, None)?;
        queue.add_crate(&BAR, &V2, 2, None)?;
        queue.add_crate(&BAZ, &V2, 2, None)?;

        assert_eq!(
            queue.pending_count_by_priority()?,
            HashMap::from_iter(vec![(1, 1), (2, 2)])
        );

        while queue.pending_count()? > 0 {
            queue.process_next_crate(|_| Ok(BuildPackageSummary::default()))?;
        }
        assert!(queue.pending_count_by_priority()?.is_empty());

        Ok(())
    }

    #[test]
    fn test_failed_count_for_reattempts() -> Result<()> {
        const MAX_ATTEMPTS: u16 = 3;

        let env = test_queue(Config {
            build_attempts: MAX_ATTEMPTS,
            delay_between_build_attempts: Duration::ZERO,
            ..Default::default()
        })?;
        let queue = &env.queue;

        assert_eq!(env.failed_count(), 0);
        queue.add_crate(&FOO, &V1, -100, None)?;
        assert_eq!(env.failed_count(), 0);
        queue.add_crate(&BAR, &V1, 0, None)?;

        for _ in 0..MAX_ATTEMPTS {
            assert_eq!(env.failed_count(), 0);
            queue.process_next_crate(|krate| {
                assert_eq!(FOO, krate.name);
                Ok(BuildPackageSummary {
                    should_reattempt: true,
                    ..Default::default()
                })
            })?;
        }
        assert_eq!(env.failed_count(), 1);

        queue.process_next_crate(|krate| {
            assert_eq!(BAR, krate.name);
            Ok(BuildPackageSummary::default())
        })?;
        assert_eq!(env.failed_count(), 1);

        Ok(())
    }

    #[test]
    fn test_failed_count_after_error() -> Result<()> {
        const MAX_ATTEMPTS: u16 = 3;

        let env = test_queue(Config {
            build_attempts: MAX_ATTEMPTS,
            delay_between_build_attempts: Duration::ZERO,
            ..Default::default()
        })?;
        let queue = &env.queue;

        assert_eq!(env.failed_count(), 0);
        queue.add_crate(&FOO, &V1, -100, None)?;
        assert_eq!(env.failed_count(), 0);
        queue.add_crate(&BAR, &V1, 0, None)?;

        for _ in 0..MAX_ATTEMPTS {
            assert_eq!(env.failed_count(), 0);
            queue.process_next_crate(|krate| {
                assert_eq!(FOO, krate.name);
                anyhow::bail!("this failed");
            })?;
        }
        assert_eq!(env.failed_count(), 1);

        queue.process_next_crate(|krate| {
            assert_eq!(BAR, krate.name);
            Ok(BuildPackageSummary::default())
        })?;
        assert_eq!(env.failed_count(), 1);

        Ok(())
    }

    #[test]
    fn test_queued_crates() -> Result<()> {
        let env = test_queue(Config::default())?;
        let queue = env.queue;

        let test_crates = [(BAR, 0), (FOO, -10), (BAZ, 10)];
        for krate in &test_crates {
            queue.add_crate(&krate.0, &V1, krate.1, None)?;
        }

        assert_eq!(
            vec![(FOO, V1, -10), (BAR, V1, 0), (BAZ, V1, 10)],
            queue
                .queued_crates()?
                .into_iter()
                .map(|c| (c.name.clone(), c.version, c.priority))
                .collect::<Vec<_>>()
        );

        Ok(())
    }

    #[test]
    fn test_queue_lock() -> Result<()> {
        let env = test_queue(Config::default())?;
        let queue = env.queue;

        // unlocked without config
        assert!(!queue.is_locked()?);

        queue.lock()?;
        assert!(queue.is_locked()?);

        queue.unlock()?;
        assert!(!queue.is_locked()?);

        Ok(())
    }

    #[test]
    fn test_add_long_name() -> Result<()> {
        let env = test_queue(Config::default())?;
        let queue = env.queue;

        let name: KrateName = "krate".repeat(100)[..64].parse().unwrap();

        queue.add_crate(&name, &V1, 0, None)?;

        queue.process_next_crate(|krate| {
            assert_eq!(name, krate.name);
            Ok(BuildPackageSummary::default())
        })?;

        Ok(())
    }

    #[test]
    fn test_add_long_version() -> Result<()> {
        let env = test_queue(Config::default())?;
        let queue = env.queue;

        let long_version = Version::parse(&format!(
            "1.2.3-{}+{}",
            "prerelease".repeat(100),
            "build".repeat(100)
        ))?;

        queue.add_crate(&KRATE, &long_version, 0, None)?;

        queue.process_next_crate(|krate| {
            assert_eq!(long_version, krate.version);
            Ok(BuildPackageSummary::default())
        })?;

        Ok(())
    }
}
