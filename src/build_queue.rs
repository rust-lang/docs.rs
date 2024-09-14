use crate::db::{delete_crate, delete_version, update_latest_version_id, Pool};
use crate::docbuilder::PackageKind;
use crate::error::Result;
use crate::storage::Storage;
use crate::utils::{get_config, get_crate_priority, report_error, retry, set_config, ConfigName};
use crate::Context;
use crate::{cdn, BuildPackageSummary};
use crate::{Config, Index, InstanceMetrics, RustwideBuilder};
use anyhow::Context as _;
use fn_error_context::context;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::runtime::Runtime;
use tracing::{debug, error, info};

#[derive(Debug, Clone, Eq, PartialEq, serde::Serialize)]
pub(crate) struct QueuedCrate {
    #[serde(skip)]
    id: i32,
    pub(crate) name: String,
    pub(crate) version: String,
    pub(crate) priority: i32,
    pub(crate) registry: Option<String>,
}

#[derive(Debug)]
pub struct BuildQueue {
    config: Arc<Config>,
    storage: Arc<Storage>,
    pub(crate) db: Pool,
    metrics: Arc<InstanceMetrics>,
    runtime: Arc<Runtime>,
    max_attempts: i32,
}

impl BuildQueue {
    pub fn new(
        db: Pool,
        metrics: Arc<InstanceMetrics>,
        config: Arc<Config>,
        storage: Arc<Storage>,
        runtime: Arc<Runtime>,
    ) -> Self {
        BuildQueue {
            max_attempts: config.build_attempts.into(),
            config,
            db,
            metrics,
            storage,
            runtime,
        }
    }

    pub fn last_seen_reference(&self) -> Result<Option<crates_index_diff::gix::ObjectId>> {
        let mut conn = self.db.get()?;
        if let Some(value) = get_config::<String>(&mut conn, ConfigName::LastSeenIndexReference)? {
            return Ok(Some(crates_index_diff::gix::ObjectId::from_hex(
                value.as_bytes(),
            )?));
        }
        Ok(None)
    }

    pub fn set_last_seen_reference(&self, oid: crates_index_diff::gix::ObjectId) -> Result<()> {
        let mut conn = self.db.get()?;
        set_config(
            &mut conn,
            ConfigName::LastSeenIndexReference,
            oid.to_string(),
        )?;
        Ok(())
    }

    #[context("error trying to add {name}-{version} to build queue")]
    pub fn add_crate(
        &self,
        name: &str,
        version: &str,
        priority: i32,
        registry: Option<&str>,
    ) -> Result<()> {
        self.db.get()?.execute(
            "INSERT INTO queue (name, version, priority, registry)
             VALUES ($1, $2, $3, $4)
             ON CONFLICT (name, version) DO UPDATE
                SET priority = EXCLUDED.priority,
                    registry = EXCLUDED.registry,
                    attempt = 0,
                    last_attempt = NULL
            ;",
            &[&name, &version, &priority, &registry],
        )?;
        Ok(())
    }

    pub(crate) fn pending_count(&self) -> Result<usize> {
        Ok(self.pending_count_by_priority()?.values().sum::<usize>())
    }

    pub(crate) fn prioritized_count(&self) -> Result<usize> {
        Ok(self
            .pending_count_by_priority()?
            .iter()
            .filter(|(&priority, _)| priority <= 0)
            .map(|(_, count)| count)
            .sum::<usize>())
    }

    pub(crate) fn pending_count_by_priority(&self) -> Result<HashMap<i32, usize>> {
        let res = self.db.get()?.query(
            "SELECT
                priority,
                COUNT(*)
            FROM queue
            WHERE attempt < $1
            GROUP BY priority",
            &[&self.max_attempts],
        )?;
        Ok(res
            .iter()
            .map(|row| (row.get::<_, i32>(0), row.get::<_, i64>(1) as usize))
            .collect())
    }

    pub(crate) fn failed_count(&self) -> Result<usize> {
        let res = self.db.get()?.query(
            "SELECT COUNT(*) FROM queue WHERE attempt >= $1;",
            &[&self.max_attempts],
        )?;
        Ok(res[0].get::<_, i64>(0) as usize)
    }

    pub(crate) fn queued_crates(&self) -> Result<Vec<QueuedCrate>> {
        let query = self.db.get()?.query(
            "SELECT id, name, version, priority, registry
             FROM queue
             WHERE attempt < $1
             ORDER BY priority ASC, attempt ASC, id ASC",
            &[&self.max_attempts],
        )?;

        Ok(query
            .into_iter()
            .map(|row| QueuedCrate {
                id: row.get("id"),
                name: row.get("name"),
                version: row.get("version"),
                priority: row.get("priority"),
                registry: row.get("registry"),
            })
            .collect())
    }

    pub(crate) fn has_build_queued(&self, name: &str, version: &str) -> Result<bool> {
        Ok(self
            .db
            .get()?
            .query_opt(
                "SELECT id
                 FROM queue
                 WHERE
                    attempt < $1 AND
                    name = $2 AND
                    version = $3
                 ",
                &[&self.max_attempts, &name, &version],
            )?
            .is_some())
    }

    fn process_next_crate(
        &self,
        f: impl FnOnce(&QueuedCrate) -> Result<BuildPackageSummary>,
    ) -> Result<()> {
        let mut conn = self.db.get()?;
        let mut transaction = conn.transaction()?;

        // fetch the next available crate from the queue table.
        // We are using `SELECT FOR UPDATE` inside a transaction so
        // the QueuedCrate is locked until we are finished with it.
        // `SKIP LOCKED` here will enable another build-server to just
        // skip over taken (=locked) rows and start building the first
        // available one.
        let to_process = match transaction
            .query_opt(
                "SELECT id, name, version, priority, registry
                 FROM queue
                 WHERE
                    attempt < $1 AND
                    (last_attempt IS NULL OR last_attempt < NOW() - make_interval(secs => $2))
                 ORDER BY priority ASC, attempt ASC, id ASC
                 LIMIT 1
                 FOR UPDATE SKIP LOCKED",
                &[
                    &self.max_attempts,
                    &self.config.delay_between_build_attempts.as_secs_f64(),
                ],
            )?
            .map(|row| QueuedCrate {
                id: row.get("id"),
                name: row.get("name"),
                version: row.get("version"),
                priority: row.get("priority"),
                registry: row.get("registry"),
            }) {
            Some(krate) => krate,
            None => return Ok(()),
        };

        let res = self
            .metrics
            .build_time
            .observe_closure_duration(|| f(&to_process));

        self.metrics.total_builds.inc();
        if let Err(err) =
            cdn::queue_crate_invalidation(&mut transaction, &self.config, &to_process.name)
        {
            report_error(&err);
        }

        let mut increase_attempt_count = || -> Result<()> {
            let attempt: i32 = transaction
                .query_one(
                    "UPDATE queue
                         SET
                            attempt = attempt + 1,
                            last_attempt = NOW()
                         WHERE id = $1
                         RETURNING attempt;",
                    &[&to_process.id],
                )?
                .get(0);

            if attempt >= self.max_attempts {
                self.metrics.failed_builds.inc();
            }
            Ok(())
        };

        match res {
            Ok(BuildPackageSummary {
                should_reattempt: false,
                successful: _,
            }) => {
                transaction.execute("DELETE FROM queue WHERE id = $1;", &[&to_process.id])?;
            }
            Ok(BuildPackageSummary {
                should_reattempt: true,
                successful: _,
            }) => {
                increase_attempt_count()?;
            }
            Err(e) => {
                increase_attempt_count()?;
                report_error(&e.context(format!(
                    "Failed to build package {}-{} from queue",
                    to_process.name, to_process.version
                )))
            }
        }

        transaction.commit()?;

        Ok(())
    }
}

/// Locking functions.
impl BuildQueue {
    /// Checks for the lock and returns whether it currently exists.
    pub fn is_locked(&self) -> Result<bool> {
        let mut conn = self.db.get()?;

        Ok(get_config::<bool>(&mut conn, ConfigName::QueueLocked)?.unwrap_or(false))
    }

    /// lock the queue. Daemon will check this lock and stop operating if it exists.
    pub fn lock(&self) -> Result<()> {
        let mut conn = self.db.get()?;
        set_config(&mut conn, ConfigName::QueueLocked, true)
    }

    /// unlock the queue.
    pub fn unlock(&self) -> Result<()> {
        let mut conn = self.db.get()?;
        set_config(&mut conn, ConfigName::QueueLocked, false)
    }
}

/// Index methods.
impl BuildQueue {
    /// Updates registry index repository and adds new crates into build queue.
    ///
    /// Returns the number of crates added
    pub fn get_new_crates(&self, index: &Index) -> Result<usize> {
        let mut conn = self.db.get()?;
        let diff = index.diff()?;

        let last_seen_reference = self
            .last_seen_reference()?
            .context("no last_seen_reference set in database")?;
        diff.set_last_seen_reference(last_seen_reference)?;

        let (changes, new_reference) = diff.peek_changes_ordered()?;
        let mut crates_added = 0;

        debug!("queueing changes from {last_seen_reference} to {new_reference}");

        for change in &changes {
            if let Some((ref krate, ..)) = change.crate_deleted() {
                match delete_crate(&mut conn, &self.storage, &self.config, krate)
                    .with_context(|| format!("failed to delete crate {krate}"))
                {
                    Ok(_) => info!(
                        "crate {} was deleted from the index and the database",
                        krate
                    ),
                    Err(err) => report_error(&err),
                }
                if let Err(err) = cdn::queue_crate_invalidation(&mut *conn, &self.config, krate) {
                    report_error(&err);
                }
                continue;
            }

            if let Some(release) = change.version_deleted() {
                match delete_version(
                    &mut conn,
                    &self.storage,
                    &self.config,
                    &release.name,
                    &release.version,
                )
                .with_context(|| {
                    format!(
                        "failed to delete version {}-{}",
                        release.name, release.version
                    )
                }) {
                    Ok(_) => info!(
                        "release {}-{} was deleted from the index and the database",
                        release.name, release.version
                    ),
                    Err(err) => report_error(&err),
                }
                if let Err(err) =
                    cdn::queue_crate_invalidation(&mut *conn, &self.config, &release.name)
                {
                    report_error(&err);
                }
                continue;
            }

            if let Some(release) = change.added() {
                let priority = get_crate_priority(&mut conn, &release.name)?;

                match self
                    .add_crate(
                        &release.name,
                        &release.version,
                        priority,
                        index.repository_url(),
                    )
                    .with_context(|| {
                        format!(
                            "failed adding {}-{} into build queue",
                            release.name, release.version
                        )
                    }) {
                    Ok(()) => {
                        debug!(
                            "{}-{} added into build queue",
                            release.name, release.version
                        );
                        self.metrics.queued_builds.inc();
                        crates_added += 1;
                    }
                    Err(err) => report_error(&err),
                }
            }

            let yanked = change.yanked();
            let unyanked = change.unyanked();
            if let Some(release) = yanked.or(unyanked) {
                // FIXME: delay yanks of crates that have not yet finished building
                // https://github.com/rust-lang/docs.rs/issues/1934
                if let Err(err) = self.set_yanked(
                    &mut conn,
                    release.name.as_str(),
                    release.version.as_str(),
                    yanked.is_some(),
                ) {
                    report_error(&err);
                }

                if let Err(err) =
                    cdn::queue_crate_invalidation(&mut *conn, &self.config, &release.name)
                {
                    report_error(&err);
                }
            }
        }

        // set the reference in the database
        // so this survives recreating the registry watcher
        // server.
        self.set_last_seen_reference(new_reference)?;

        Ok(crates_added)
    }

    #[context("error trying to set {name}-{version} to yanked: {yanked}")]
    pub fn set_yanked(
        &self,
        conn: &mut postgres::Client,
        name: &str,
        version: &str,
        yanked: bool,
    ) -> Result<()> {
        let activity = if yanked { "yanked" } else { "unyanked" };

        let result = conn.query(
            "UPDATE releases
             SET yanked = $3
             FROM crates
             WHERE crates.id = releases.crate_id
                 AND name = $1
                 AND version = $2
            RETURNING crates.id
            ",
            &[&name, &version, &yanked],
        )?;
        if result.len() != 1 {
            match self
                .has_build_queued(name, version)
                .context("error trying to fetch build queue")
            {
                Ok(false) => {
                    // the rustwide builder will fetch the current yank state from
                    // crates.io, so and missed update here will be fixed after the
                    // build is finished.
                    error!(
                        "tried to yank or unyank non-existing release: {} {}",
                        name, version
                    );
                }
                Ok(true) => {}
                Err(err) => {
                    report_error(&err);
                }
            }
        } else {
            debug!("{}-{} {}", name, version, activity);
        }

        if let Some(row) = result.first() {
            let crate_id: i32 = row.get(0);

            self.runtime.block_on(async {
                let mut conn = self.db.get_async().await?;

                update_latest_version_id(&mut conn, crate_id).await
            })?;
        }

        Ok(())
    }

    fn update_toolchain(&self, builder: &mut RustwideBuilder) -> Result<()> {
        let updated = retry(
            || {
                builder
                    .update_toolchain()
                    .context("downloading new toolchain failed")
            },
            3,
        )?;

        if updated {
            // toolchain has changed, purge caches
            retry(
                || {
                    builder
                        .purge_caches()
                        .context("purging rustwide caches failed")
                },
                3,
            )?;

            builder
                .add_essential_files()
                .context("adding essential files failed")?;
        }

        Ok(())
    }

    /// Builds the top package from the queue. Returns whether there was a package in the queue.
    ///
    /// Note that this will return `Ok(true)` even if the package failed to build.
    pub(crate) fn build_next_queue_package(
        &self,
        context: &dyn Context,
        builder: &mut RustwideBuilder,
    ) -> Result<bool> {
        let mut processed = false;
        self.process_next_crate(|krate| {
            processed = true;

            let kind = krate
                .registry
                .as_ref()
                .map(|r| PackageKind::Registry(r.as_str()))
                .unwrap_or(PackageKind::CratesIo);

            if let Err(err) = retry(
                || {
                    builder
                        .reinitialize_workspace_if_interval_passed(context)
                        .context("Reinitialize workspace failed, locking queue")
                },
                3,
            ) {
                report_error(&err);
                self.lock()?;
                return Err(err);
            }

            if let Err(err) = self
                .update_toolchain(&mut *builder)
                .context("Updating toolchain failed, locking queue")
            {
                report_error(&err);
                self.lock()?;
                return Err(err);
            }

            builder.build_package(&krate.name, &krate.version, kind)
        })?;

        Ok(processed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{DateTime, Utc};
    use std::time::Duration;

    #[test]
    fn test_add_duplicate_doesnt_fail_last_priority_wins() {
        crate::test::wrapper(|env| {
            let queue = env.build_queue();

            queue.add_crate("some_crate", "0.1.1", 0, None)?;
            queue.add_crate("some_crate", "0.1.1", 9, None)?;

            let queued_crates = queue.queued_crates()?;
            assert_eq!(queued_crates.len(), 1);
            assert_eq!(queued_crates[0].priority, 9);

            Ok(())
        })
    }

    #[test]
    fn test_add_duplicate_resets_attempts_and_priority() {
        crate::test::wrapper(|env| {
            env.override_config(|config| {
                config.build_attempts = 5;
            });

            let queue = env.build_queue();

            let mut conn = env.db().conn();
            conn.execute(
                "
                INSERT INTO queue (name, version, priority, attempt, last_attempt )
                VALUES ('failed_crate', '0.1.1', 0, 99, NOW())",
                &[],
            )?;

            assert_eq!(queue.pending_count()?, 0);

            queue.add_crate("failed_crate", "0.1.1", 9, None)?;

            assert_eq!(queue.pending_count()?, 1);

            let row = conn
                .query_opt(
                    "SELECT priority, attempt, last_attempt
                     FROM queue
                     WHERE name = $1 AND version = $2",
                    &[&"failed_crate", &"0.1.1"],
                )?
                .unwrap();
            assert_eq!(row.get::<_, i32>(0), 9);
            assert_eq!(row.get::<_, i32>(1), 0);
            assert!(row.get::<_, Option<DateTime<Utc>>>(2).is_none());
            Ok(())
        })
    }

    #[test]
    fn test_has_build_queued() {
        crate::test::wrapper(|env| {
            let queue = env.build_queue();

            queue.add_crate("dummy", "0.1.1", 0, None)?;
            assert!(queue.has_build_queued("dummy", "0.1.1")?);

            env.db()
                .conn()
                .execute("UPDATE queue SET attempt = 6", &[])?;

            assert!(!queue.has_build_queued("dummy", "0.1.1")?);

            Ok(())
        })
    }

    #[test]
    fn test_wait_between_build_attempts() {
        crate::test::wrapper(|env| {
            env.override_config(|config| {
                config.build_attempts = 99;
                config.delay_between_build_attempts = Duration::from_secs(1);
            });

            let queue = env.build_queue();

            queue.add_crate("krate", "1.0.0", 0, None)?;

            // first let it fail
            queue.process_next_crate(|krate| {
                assert_eq!(krate.name, "krate");
                anyhow::bail!("simulate a failure");
            })?;

            queue.process_next_crate(|_| {
                // this can't happen since we didn't wait between attempts
                unreachable!();
            })?;

            {
                // fake the build-attempt timestamp so it's older
                let mut conn = env.db().conn();
                conn.execute(
                    "UPDATE queue SET last_attempt = $1",
                    &[&(Utc::now() - chrono::Duration::try_seconds(60).unwrap())],
                )?;
            }

            let mut handled = false;
            // now we can process it again
            queue.process_next_crate(|krate| {
                assert_eq!(krate.name, "krate");
                handled = true;
                Ok(BuildPackageSummary::default())
            })?;

            assert!(handled);

            Ok(())
        })
    }

    #[test]
    fn test_add_and_process_crates() {
        const MAX_ATTEMPTS: u16 = 3;

        crate::test::wrapper(|env| {
            env.override_config(|config| {
                config.build_attempts = MAX_ATTEMPTS;
                config.delay_between_build_attempts = Duration::ZERO;
            });

            let queue = env.build_queue();

            let test_crates = [
                ("low-priority", "1.0.0", 1000),
                ("high-priority-foo", "1.0.0", -1000),
                ("medium-priority", "1.0.0", -10),
                ("high-priority-bar", "1.0.0", -1000),
                ("standard-priority", "1.0.0", 0),
                ("high-priority-baz", "1.0.0", -1000),
            ];
            for krate in &test_crates {
                queue.add_crate(krate.0, krate.1, krate.2, None)?;
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
            assert_next("high-priority-foo")?;

            // Simulate a failure in high-priority-bar.
            assert_next_and_fail("high-priority-bar")?;

            // Continue with the next high priority crate.
            assert_next("high-priority-baz")?;

            // After all the crates with the max priority are processed, before starting to process
            // crates with a lower priority the failed crates with the max priority will be tried
            // again.
            assert_next("high-priority-bar")?;

            // Continue processing according to the priority.
            assert_next("medium-priority")?;
            assert_next("standard-priority")?;

            // Simulate the crate failing many times.
            for _ in 0..MAX_ATTEMPTS {
                assert_next_and_fail("low-priority")?;
            }

            // Since low-priority failed many times it will be removed from the queue. Because of
            // that the queue should now be empty.
            let mut called = false;
            queue.process_next_crate(|_| {
                called = true;
                Ok(BuildPackageSummary::default())
            })?;
            assert!(!called, "there were still items in the queue");

            // Ensure metrics were recorded correctly
            let metrics = env.instance_metrics();
            assert_eq!(metrics.total_builds.get(), 9);
            assert_eq!(metrics.failed_builds.get(), 1);
            assert_eq!(metrics.build_time.get_sample_count(), 9);

            // no invalidations were run since we don't have a distribution id configured
            assert!(cdn::queued_or_active_crate_invalidations(&mut *env.db().conn())?.is_empty());

            Ok(())
        })
    }

    #[test]
    fn test_invalidate_cdn_after_build_and_error() {
        crate::test::wrapper(|env| {
            env.override_config(|config| {
                config.cloudfront_distribution_id_web = Some("distribution_id_web".into());
                config.cloudfront_distribution_id_static = Some("distribution_id_static".into());
            });

            let queue = env.build_queue();

            queue.add_crate("will_succeed", "1.0.0", -1, None)?;
            queue.add_crate("will_fail", "1.0.0", 0, None)?;

            let mut conn = env.db().conn();
            cdn::queued_or_active_crate_invalidations(&mut *conn)?.is_empty();

            queue.process_next_crate(|krate| {
                assert_eq!("will_succeed", krate.name);
                Ok(BuildPackageSummary::default())
            })?;

            let queued_invalidations = cdn::queued_or_active_crate_invalidations(&mut *conn)?;
            assert_eq!(queued_invalidations.len(), 3);
            assert!(queued_invalidations
                .iter()
                .all(|i| i.krate == "will_succeed"));

            queue.process_next_crate(|krate| {
                assert_eq!("will_fail", krate.name);
                anyhow::bail!("simulate a failure");
            })?;

            let queued_invalidations = cdn::queued_or_active_crate_invalidations(&mut *conn)?;
            assert_eq!(queued_invalidations.len(), 6);
            assert!(queued_invalidations
                .iter()
                .skip(3)
                .all(|i| i.krate == "will_fail"));

            Ok(())
        })
    }

    #[test]
    fn test_pending_count() {
        crate::test::wrapper(|env| {
            let queue = env.build_queue();

            assert_eq!(queue.pending_count()?, 0);
            queue.add_crate("foo", "1.0.0", 0, None)?;
            assert_eq!(queue.pending_count()?, 1);
            queue.add_crate("bar", "1.0.0", 0, None)?;
            assert_eq!(queue.pending_count()?, 2);

            queue.process_next_crate(|krate| {
                assert_eq!("foo", krate.name);
                Ok(BuildPackageSummary::default())
            })?;
            assert_eq!(queue.pending_count()?, 1);

            Ok(())
        });
    }

    #[test]
    fn test_prioritized_count() {
        crate::test::wrapper(|env| {
            let queue = env.build_queue();

            assert_eq!(queue.prioritized_count()?, 0);
            queue.add_crate("foo", "1.0.0", 0, None)?;
            assert_eq!(queue.prioritized_count()?, 1);
            queue.add_crate("bar", "1.0.0", -100, None)?;
            assert_eq!(queue.prioritized_count()?, 2);
            queue.add_crate("baz", "1.0.0", 100, None)?;
            assert_eq!(queue.prioritized_count()?, 2);

            queue.process_next_crate(|krate| {
                assert_eq!("bar", krate.name);
                Ok(BuildPackageSummary::default())
            })?;
            assert_eq!(queue.prioritized_count()?, 1);

            Ok(())
        });
    }

    #[test]
    fn test_count_by_priority() {
        crate::test::wrapper(|env| {
            let queue = env.build_queue();

            assert!(queue.pending_count_by_priority()?.is_empty());

            queue.add_crate("one", "1.0.0", 1, None)?;
            queue.add_crate("two", "2.0.0", 2, None)?;
            queue.add_crate("two_more", "2.0.0", 2, None)?;

            assert_eq!(
                queue.pending_count_by_priority()?,
                HashMap::from_iter(vec![(1, 1), (2, 2)])
            );

            while queue.pending_count()? > 0 {
                queue.process_next_crate(|_| Ok(BuildPackageSummary::default()))?;
            }
            assert!(queue.pending_count_by_priority()?.is_empty());

            Ok(())
        });
    }

    #[test]
    fn test_failed_count_for_reattempts() {
        const MAX_ATTEMPTS: u16 = 3;
        crate::test::wrapper(|env| {
            env.override_config(|config| {
                config.build_attempts = MAX_ATTEMPTS;
                config.delay_between_build_attempts = Duration::ZERO;
            });
            let queue = env.build_queue();

            assert_eq!(queue.failed_count()?, 0);
            queue.add_crate("foo", "1.0.0", -100, None)?;
            assert_eq!(queue.failed_count()?, 0);
            queue.add_crate("bar", "1.0.0", 0, None)?;

            for _ in 0..MAX_ATTEMPTS {
                assert_eq!(queue.failed_count()?, 0);
                queue.process_next_crate(|krate| {
                    assert_eq!("foo", krate.name);
                    Ok(BuildPackageSummary {
                        should_reattempt: true,
                        ..Default::default()
                    })
                })?;
            }
            assert_eq!(queue.failed_count()?, 1);

            queue.process_next_crate(|krate| {
                assert_eq!("bar", krate.name);
                Ok(BuildPackageSummary::default())
            })?;
            assert_eq!(queue.failed_count()?, 1);

            Ok(())
        });
    }

    #[test]
    fn test_failed_count_after_error() {
        const MAX_ATTEMPTS: u16 = 3;
        crate::test::wrapper(|env| {
            env.override_config(|config| {
                config.build_attempts = MAX_ATTEMPTS;
                config.delay_between_build_attempts = Duration::ZERO;
            });
            let queue = env.build_queue();

            assert_eq!(queue.failed_count()?, 0);
            queue.add_crate("foo", "1.0.0", -100, None)?;
            assert_eq!(queue.failed_count()?, 0);
            queue.add_crate("bar", "1.0.0", 0, None)?;

            for _ in 0..MAX_ATTEMPTS {
                assert_eq!(queue.failed_count()?, 0);
                queue.process_next_crate(|krate| {
                    assert_eq!("foo", krate.name);
                    anyhow::bail!("this failed");
                })?;
            }
            assert_eq!(queue.failed_count()?, 1);

            queue.process_next_crate(|krate| {
                assert_eq!("bar", krate.name);
                Ok(BuildPackageSummary::default())
            })?;
            assert_eq!(queue.failed_count()?, 1);

            Ok(())
        });
    }

    #[test]
    fn test_queued_crates() {
        crate::test::wrapper(|env| {
            let queue = env.build_queue();

            let test_crates = [
                ("bar", "1.0.0", 0),
                ("foo", "1.0.0", -10),
                ("baz", "1.0.0", 10),
            ];
            for krate in &test_crates {
                queue.add_crate(krate.0, krate.1, krate.2, None)?;
            }

            assert_eq!(
                vec![
                    ("foo", "1.0.0", -10),
                    ("bar", "1.0.0", 0),
                    ("baz", "1.0.0", 10),
                ],
                queue
                    .queued_crates()?
                    .iter()
                    .map(|c| (c.name.as_str(), c.version.as_str(), c.priority))
                    .collect::<Vec<_>>()
            );

            Ok(())
        });
    }

    #[test]
    fn test_last_seen_reference_in_db() {
        crate::test::wrapper(|env| {
            let queue = env.build_queue();
            queue.unlock()?;
            assert!(!queue.is_locked()?);
            // initial db ref is empty
            assert_eq!(queue.last_seen_reference()?, None);
            assert!(!queue.is_locked()?);

            let oid = crates_index_diff::gix::ObjectId::from_hex(
                b"ffffffffffffffffffffffffffffffffffffffff",
            )?;
            queue.set_last_seen_reference(oid)?;

            assert_eq!(queue.last_seen_reference()?, Some(oid));
            assert!(!queue.is_locked()?);

            Ok(())
        });
    }

    #[test]
    fn test_broken_db_reference_breaks() {
        crate::test::wrapper(|env| {
            let mut conn = env.db().conn();
            set_config(&mut conn, ConfigName::LastSeenIndexReference, "invalid")?;

            let queue = env.build_queue();
            assert!(queue.last_seen_reference().is_err());

            Ok(())
        });
    }

    #[test]
    fn test_queue_lock() {
        crate::test::wrapper(|env| {
            let queue = env.build_queue();
            // unlocked without config
            assert!(!queue.is_locked()?);

            queue.lock()?;
            assert!(queue.is_locked()?);

            queue.unlock()?;
            assert!(!queue.is_locked()?);

            Ok(())
        });
    }
}
