use crate::Context;
use crate::db::{CrateId, Pool, delete_crate, delete_version, update_latest_version_id};
use crate::docbuilder::PackageKind;
use crate::error::Result;
use crate::storage::AsyncStorage;
use crate::utils::{ConfigName, get_config, get_crate_priority, report_error, retry, set_config};
use crate::{BuildPackageSummary, cdn};
use crate::{Config, Index, InstanceMetrics, RustwideBuilder};
use anyhow::Context as _;
use fn_error_context::context;
use futures_util::{StreamExt, stream::TryStreamExt};
use sqlx::Connection as _;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::runtime::Runtime;
use tracing::{debug, error, info, instrument};

/// The static priority for background rebuilds.
/// Used when queueing rebuilds, and when rendering them
/// collapsed in the UI.
/// For normal build priorities we use smaller values.
pub(crate) const REBUILD_PRIORITY: i32 = 20;

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
pub struct AsyncBuildQueue {
    config: Arc<Config>,
    storage: Arc<AsyncStorage>,
    pub(crate) db: Pool,
    metrics: Arc<InstanceMetrics>,
    max_attempts: i32,
}

impl AsyncBuildQueue {
    pub fn new(
        db: Pool,
        metrics: Arc<InstanceMetrics>,
        config: Arc<Config>,
        storage: Arc<AsyncStorage>,
    ) -> Self {
        AsyncBuildQueue {
            max_attempts: config.build_attempts.into(),
            config,
            db,
            metrics,
            storage,
        }
    }

    pub async fn last_seen_reference(&self) -> Result<Option<crates_index_diff::gix::ObjectId>> {
        let mut conn = self.db.get_async().await?;
        if let Some(value) =
            get_config::<String>(&mut conn, ConfigName::LastSeenIndexReference).await?
        {
            return Ok(Some(crates_index_diff::gix::ObjectId::from_hex(
                value.as_bytes(),
            )?));
        }
        Ok(None)
    }

    pub async fn set_last_seen_reference(
        &self,
        oid: crates_index_diff::gix::ObjectId,
    ) -> Result<()> {
        let mut conn = self.db.get_async().await?;
        set_config(
            &mut conn,
            ConfigName::LastSeenIndexReference,
            oid.to_string(),
        )
        .await?;
        Ok(())
    }

    #[context("error trying to add {name}-{version} to build queue")]
    pub async fn add_crate(
        &self,
        name: &str,
        version: &str,
        priority: i32,
        registry: Option<&str>,
    ) -> Result<()> {
        let mut conn = self.db.get_async().await?;

        sqlx::query!(
            "INSERT INTO queue (name, version, priority, registry)
                 VALUES ($1, $2, $3, $4)
                 ON CONFLICT (name, version) DO UPDATE
                    SET priority = EXCLUDED.priority,
                        registry = EXCLUDED.registry,
                        attempt = 0,
                        last_attempt = NULL
                ;",
            name,
            version,
            priority,
            registry,
        )
        .execute(&mut *conn)
        .await?;

        Ok(())
    }

    pub(crate) async fn pending_count(&self) -> Result<usize> {
        Ok(self
            .pending_count_by_priority()
            .await?
            .values()
            .sum::<usize>())
    }

    pub(crate) async fn prioritized_count(&self) -> Result<usize> {
        Ok(self
            .pending_count_by_priority()
            .await?
            .iter()
            .filter(|&(&priority, _)| priority <= 0)
            .map(|(_, count)| count)
            .sum::<usize>())
    }

    pub(crate) async fn pending_count_by_priority(&self) -> Result<HashMap<i32, usize>> {
        let mut conn = self.db.get_async().await?;

        Ok(sqlx::query!(
            r#"
                SELECT
                    priority,
                    COUNT(*) as "count!"
                FROM queue
                WHERE attempt < $1
                GROUP BY priority"#,
            self.max_attempts,
        )
        .fetch(&mut *conn)
        .map_ok(|row| (row.priority, row.count as usize))
        .try_collect()
        .await?)
    }

    pub(crate) async fn failed_count(&self) -> Result<usize> {
        let mut conn = self.db.get_async().await?;

        Ok(sqlx::query_scalar!(
            r#"SELECT COUNT(*) as "count!" FROM queue WHERE attempt >= $1;"#,
            self.max_attempts,
        )
        .fetch_one(&mut *conn)
        .await? as usize)
    }

    pub(crate) async fn queued_crates(&self) -> Result<Vec<QueuedCrate>> {
        let mut conn = self.db.get_async().await?;

        Ok(sqlx::query_as!(
            QueuedCrate,
            "SELECT id, name, version, priority, registry
                 FROM queue
                 WHERE attempt < $1
                 ORDER BY priority ASC, attempt ASC, id ASC",
            self.max_attempts
        )
        .fetch_all(&mut *conn)
        .await?)
    }

    pub(crate) async fn has_build_queued(&self, name: &str, version: &str) -> Result<bool> {
        let mut conn = self.db.get_async().await?;
        Ok(sqlx::query_scalar!(
            "SELECT id
             FROM queue
             WHERE
                attempt < $1 AND
                name = $2 AND
                version = $3
             ",
            self.max_attempts,
            name,
            version,
        )
        .fetch_optional(&mut *conn)
        .await?
        .is_some())
    }
}

/// Locking functions.
impl AsyncBuildQueue {
    /// Checks for the lock and returns whether it currently exists.
    pub async fn is_locked(&self) -> Result<bool> {
        let mut conn = self.db.get_async().await?;

        Ok(get_config::<bool>(&mut conn, ConfigName::QueueLocked)
            .await?
            .unwrap_or(false))
    }

    /// lock the queue. Daemon will check this lock and stop operating if it exists.
    pub async fn lock(&self) -> Result<()> {
        let mut conn = self.db.get_async().await?;
        set_config(&mut conn, ConfigName::QueueLocked, true).await
    }

    /// unlock the queue.
    pub async fn unlock(&self) -> Result<()> {
        let mut conn = self.db.get_async().await?;
        set_config(&mut conn, ConfigName::QueueLocked, false).await
    }
}

/// Index methods.
impl AsyncBuildQueue {
    /// Updates registry index repository and adds new crates into build queue.
    ///
    /// Returns the number of crates added
    pub async fn get_new_crates(&self, index: &Index) -> Result<usize> {
        let diff = index.diff()?;

        let last_seen_reference = self
            .last_seen_reference()
            .await?
            .context("no last_seen_reference set in database")?;
        diff.set_last_seen_reference(last_seen_reference)?;

        let (changes, new_reference) = diff.peek_changes_ordered()?;

        let mut conn = self.db.get_async().await?;
        let mut crates_added = 0;

        debug!("queueing changes from {last_seen_reference} to {new_reference}");

        for change in &changes {
            if let Some((ref krate, ..)) = change.crate_deleted() {
                match delete_crate(&mut conn, &self.storage, &self.config, krate)
                    .await
                    .with_context(|| format!("failed to delete crate {krate}"))
                {
                    Ok(_) => info!(
                        "crate {} was deleted from the index and the database",
                        krate
                    ),
                    Err(err) => report_error(&err),
                }
                if let Err(err) =
                    cdn::queue_crate_invalidation(&mut conn, &self.config, krate).await
                {
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
                .await
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
                    cdn::queue_crate_invalidation(&mut conn, &self.config, &release.name).await
                {
                    report_error(&err);
                }
                continue;
            }

            if let Some(release) = change.added() {
                let priority = get_crate_priority(&mut conn, &release.name).await?;

                match self
                    .add_crate(
                        &release.name,
                        &release.version,
                        priority,
                        index.repository_url(),
                    )
                    .await
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
                if let Err(err) = self
                    .set_yanked_inner(
                        &mut conn,
                        release.name.as_str(),
                        release.version.as_str(),
                        yanked.is_some(),
                    )
                    .await
                {
                    report_error(&err);
                }

                if let Err(err) =
                    cdn::queue_crate_invalidation(&mut conn, &self.config, &release.name).await
                {
                    report_error(&err);
                }
            }
        }

        // set the reference in the database
        // so this survives recreating the registry watcher
        // server.
        self.set_last_seen_reference(new_reference).await?;

        Ok(crates_added)
    }

    pub async fn set_yanked(&self, name: &str, version: &str, yanked: bool) -> Result<()> {
        let mut conn = self.db.get_async().await?;
        self.set_yanked_inner(&mut conn, name, version, yanked)
            .await
    }

    #[context("error trying to set {name}-{version} to yanked: {yanked}")]
    async fn set_yanked_inner(
        &self,
        conn: &mut sqlx::PgConnection,
        name: &str,
        version: &str,
        yanked: bool,
    ) -> Result<()> {
        let activity = if yanked { "yanked" } else { "unyanked" };

        if let Some(crate_id) = sqlx::query_scalar!(
            r#"UPDATE releases
             SET yanked = $3
             FROM crates
             WHERE crates.id = releases.crate_id
                 AND name = $1
                 AND version = $2
            RETURNING crates.id as "id: CrateId"
            "#,
            name,
            version,
            yanked,
        )
        .fetch_optional(&mut *conn)
        .await?
        {
            debug!("{}-{} {}", name, version, activity);
            update_latest_version_id(&mut *conn, crate_id).await?;
        } else {
            match self
                .has_build_queued(name, version)
                .await
                .context("error trying to fetch build queue")
            {
                Ok(false) => {
                    error!(
                        "tried to yank or unyank non-existing release: {} {}",
                        name, version
                    );
                }
                Ok(true) => {
                    // the rustwide builder will fetch the current yank state from
                    // crates.io, so and missed update here will be fixed after the
                    // build is finished.
                }
                Err(err) => {
                    report_error(&err);
                }
            }
        }

        Ok(())
    }
}

#[derive(Debug)]
pub struct BuildQueue {
    runtime: Arc<Runtime>,
    inner: Arc<AsyncBuildQueue>,
}

/// sync versions of async methods
impl BuildQueue {
    pub fn add_crate(
        &self,
        name: &str,
        version: &str,
        priority: i32,
        registry: Option<&str>,
    ) -> Result<()> {
        self.runtime
            .block_on(self.inner.add_crate(name, version, priority, registry))
    }

    pub fn set_yanked(&self, name: &str, version: &str, yanked: bool) -> Result<()> {
        self.runtime
            .block_on(self.inner.set_yanked(name, version, yanked))
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
    pub fn last_seen_reference(&self) -> Result<Option<crates_index_diff::gix::ObjectId>> {
        self.runtime.block_on(self.inner.last_seen_reference())
    }
    pub fn set_last_seen_reference(&self, oid: crates_index_diff::gix::ObjectId) -> Result<()> {
        self.runtime
            .block_on(self.inner.set_last_seen_reference(oid))
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
    pub(crate) fn failed_count(&self) -> Result<usize> {
        self.runtime.block_on(self.inner.failed_count())
    }
    #[cfg(test)]
    pub(crate) fn queued_crates(&self) -> Result<Vec<QueuedCrate>> {
        self.runtime.block_on(self.inner.queued_crates())
    }
}

impl BuildQueue {
    pub fn new(runtime: Arc<Runtime>, inner: Arc<AsyncBuildQueue>) -> Self {
        Self { runtime, inner }
    }

    fn process_next_crate(
        &self,
        f: impl FnOnce(&QueuedCrate) -> Result<BuildPackageSummary>,
    ) -> Result<()> {
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
                "SELECT id, name, version, priority, registry
                 FROM queue
                 WHERE
                    attempt < $1 AND
                    (last_attempt IS NULL OR last_attempt < NOW() - make_interval(secs => $2))
                 ORDER BY priority ASC, attempt ASC, id ASC
                 LIMIT 1
                 FOR UPDATE SKIP LOCKED",
                self.inner.max_attempts,
                self.inner.config.delay_between_build_attempts.as_secs_f64(),
            )
            .fetch_optional(&mut *transaction),
        )? {
            Some(krate) => krate,
            None => return Ok(()),
        };

        let res = self
            .inner
            .metrics
            .build_time
            .observe_closure_duration(|| f(&to_process));

        self.inner.metrics.total_builds.inc();
        if let Err(err) = self.runtime.block_on(cdn::queue_crate_invalidation(
            &mut transaction,
            &self.inner.config,
            &to_process.name,
        )) {
            report_error(&err);
        }

        let mut increase_attempt_count = || -> Result<()> {
            let attempt: i32 = self.runtime.block_on(
                sqlx::query_scalar!(
                    "UPDATE queue
                         SET
                            attempt = attempt + 1,
                            last_attempt = NOW()
                         WHERE id = $1
                         RETURNING attempt;",
                    to_process.id,
                )
                .fetch_one(&mut *transaction),
            )?;

            if attempt >= self.inner.max_attempts {
                self.inner.metrics.failed_builds.inc();
            }
            Ok(())
        };

        match res {
            Ok(BuildPackageSummary {
                should_reattempt: false,
                successful: _,
            }) => {
                self.runtime.block_on(
                    sqlx::query!("DELETE FROM queue WHERE id = $1;", to_process.id)
                        .execute(&mut *transaction),
                )?;
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

        self.runtime.block_on(transaction.commit())?;
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
    pub(crate) fn build_next_queue_package<C: Context>(
        &self,
        context: &C,
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

/// Queue rebuilds as configured.
///
/// The idea is to rebuild:
/// * the latest release of each crate
/// * when the nightly version is older than our configured threshold
/// * and there was a successful build for that release, that included documentation.
/// * starting with the oldest nightly versions.
/// * also checking if there is already a build queued.
///
/// This might exclude releases from rebuilds that
/// * previously failed but would succeed with a newer nightly version
/// * previously failed but would succeed just with a retry.
#[instrument(skip_all)]
pub async fn queue_rebuilds(
    conn: &mut sqlx::PgConnection,
    config: &Config,
    build_queue: &AsyncBuildQueue,
) -> Result<()> {
    let already_queued_rebuilds: usize = build_queue
        .pending_count_by_priority()
        .await?
        .iter()
        .filter_map(|(priority, count)| (*priority >= REBUILD_PRIORITY).then_some(count))
        .sum();

    let rebuilds_to_queue = config
        .max_queued_rebuilds
        .expect("config.max_queued_rebuilds not set") as i64
        - already_queued_rebuilds as i64;

    if rebuilds_to_queue <= 0 {
        info!("not queueing rebuilds; queue limit reached");
        return Ok(());
    }

    let mut results = sqlx::query!(
        "SELECT i.* FROM (
             SELECT
                 c.name,
                 r.version,
                 (
                    SELECT MAX(COALESCE(b.build_finished, b.build_started))
                    FROM builds AS b
                    WHERE b.rid = r.id
                 ) AS last_build_attempt
             FROM crates AS c
             INNER JOIN releases AS r ON c.latest_version_id = r.id

             WHERE
                 r.rustdoc_status = TRUE
         ) as i
         ORDER BY i.last_build_attempt ASC
         LIMIT $1",
        rebuilds_to_queue,
    )
    .fetch(&mut *conn);

    while let Some(row) = results.next().await {
        let row = row?;

        if !build_queue
            .has_build_queued(&row.name, &row.version)
            .await?
        {
            info!("queueing rebuild for {} {}...", &row.name, &row.version);
            build_queue
                .add_crate(&row.name, &row.version, REBUILD_PRIORITY, None)
                .await?;
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use crate::test::FakeBuild;

    use super::*;
    use chrono::Utc;
    use std::time::Duration;

    #[test]
    fn test_rebuild_when_old() {
        crate::test::async_wrapper(|env| async move {
            env.override_config(|config| {
                config.max_queued_rebuilds = Some(100);
            });

            env.fake_release()
                .await
                .name("foo")
                .version("0.1.0")
                .builds(vec![
                    FakeBuild::default()
                        .rustc_version("rustc 1.84.0-nightly (e7c0d2750 2020-10-15)"),
                ])
                .create()
                .await?;

            let build_queue = env.async_build_queue().await;
            assert!(build_queue.queued_crates().await?.is_empty());

            let mut conn = env.async_db().await.async_conn().await;
            queue_rebuilds(&mut conn, &env.config(), &build_queue).await?;

            let queue = build_queue.queued_crates().await?;
            assert_eq!(queue.len(), 1);
            assert_eq!(queue[0].name, "foo");
            assert_eq!(queue[0].version, "0.1.0");
            assert_eq!(queue[0].priority, REBUILD_PRIORITY);

            Ok(())
        })
    }

    #[test]
    fn test_still_rebuild_when_full_with_failed() {
        crate::test::async_wrapper(|env| async move {
            env.override_config(|config| {
                config.max_queued_rebuilds = Some(1);
            });

            let build_queue = env.async_build_queue().await;
            build_queue
                .add_crate("foo1", "0.1.0", REBUILD_PRIORITY, None)
                .await?;
            build_queue
                .add_crate("foo2", "0.1.0", REBUILD_PRIORITY, None)
                .await?;

            let mut conn = env.async_db().await.async_conn().await;
            sqlx::query!("UPDATE queue SET attempt = 99")
                .execute(&mut *conn)
                .await?;

            assert_eq!(build_queue.queued_crates().await?.len(), 0);

            env.fake_release()
                .await
                .name("foo")
                .version("0.1.0")
                .builds(vec![
                    FakeBuild::default()
                        .rustc_version("rustc 1.84.0-nightly (e7c0d2750 2020-10-15)"),
                ])
                .create()
                .await?;

            let build_queue = env.async_build_queue().await;
            queue_rebuilds(&mut conn, &env.config(), &build_queue).await?;

            assert_eq!(build_queue.queued_crates().await?.len(), 1);

            Ok(())
        })
    }

    #[test]
    fn test_dont_rebuild_when_full() {
        crate::test::async_wrapper(|env| async move {
            env.override_config(|config| {
                config.max_queued_rebuilds = Some(1);
            });

            let build_queue = env.async_build_queue().await;
            build_queue
                .add_crate("foo1", "0.1.0", REBUILD_PRIORITY, None)
                .await?;
            build_queue
                .add_crate("foo2", "0.1.0", REBUILD_PRIORITY, None)
                .await?;

            env.fake_release()
                .await
                .name("foo")
                .version("0.1.0")
                .builds(vec![
                    FakeBuild::default()
                        .rustc_version("rustc 1.84.0-nightly (e7c0d2750 2020-10-15)"),
                ])
                .create()
                .await?;

            let build_queue = env.async_build_queue().await;
            assert_eq!(build_queue.queued_crates().await?.len(), 2);

            let mut conn = env.async_db().await.async_conn().await;
            queue_rebuilds(&mut conn, &env.config(), &build_queue).await?;

            assert_eq!(build_queue.queued_crates().await?.len(), 2);

            Ok(())
        })
    }

    #[test]
    fn test_add_duplicate_doesnt_fail_last_priority_wins() {
        crate::test::async_wrapper(|env| async move {
            let queue = env.async_build_queue().await;

            queue.add_crate("some_crate", "0.1.1", 0, None).await?;
            queue.add_crate("some_crate", "0.1.1", 9, None).await?;

            let queued_crates = queue.queued_crates().await?;
            assert_eq!(queued_crates.len(), 1);
            assert_eq!(queued_crates[0].priority, 9);

            Ok(())
        })
    }

    #[test]
    fn test_add_duplicate_resets_attempts_and_priority() {
        crate::test::async_wrapper(|env| async move {
            env.override_config(|config| {
                config.build_attempts = 5;
            });

            let queue = env.async_build_queue().await;

            let mut conn = env.async_db().await.async_conn().await;
            sqlx::query!(
                "
                INSERT INTO queue (name, version, priority, attempt, last_attempt )
                VALUES ('failed_crate', '0.1.1', 0, 99, NOW())",
            )
            .execute(&mut *conn)
            .await?;

            assert_eq!(queue.pending_count().await?, 0);

            queue.add_crate("failed_crate", "0.1.1", 9, None).await?;

            assert_eq!(queue.pending_count().await?, 1);

            let row = sqlx::query!(
                "SELECT priority, attempt, last_attempt
                     FROM queue
                     WHERE name = $1 AND version = $2",
                "failed_crate",
                "0.1.1",
            )
            .fetch_one(&mut *conn)
            .await?;

            assert_eq!(row.priority, 9);
            assert_eq!(row.attempt, 0);
            assert!(row.last_attempt.is_none());
            Ok(())
        })
    }

    #[test]
    fn test_has_build_queued() {
        crate::test::async_wrapper(|env| async move {
            let queue = env.async_build_queue().await;

            queue.add_crate("dummy", "0.1.1", 0, None).await?;

            let mut conn = env.async_db().await.async_conn().await;
            assert!(queue.has_build_queued("dummy", "0.1.1").await.unwrap());

            sqlx::query!("UPDATE queue SET attempt = 6")
                .execute(&mut *conn)
                .await
                .unwrap();

            assert!(!queue.has_build_queued("dummy", "0.1.1").await.unwrap());

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

            let runtime = env.runtime();

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

            runtime.block_on(async {
                // fake the build-attempt timestamp so it's older
                let mut conn = env.async_db().await.async_conn().await;
                sqlx::query!(
                    "UPDATE queue SET last_attempt = $1",
                    Utc::now() - chrono::Duration::try_seconds(60).unwrap()
                )
                .execute(&mut *conn)
                .await
            })?;

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
            assert!(
                env.runtime()
                    .block_on(async {
                        cdn::queued_or_active_crate_invalidations(
                            &mut *env.async_db().await.async_conn().await,
                        )
                        .await
                    })?
                    .is_empty()
            );

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

            let fetch_invalidations = || {
                env.runtime()
                    .block_on(async {
                        let mut conn = env.async_db().await.async_conn().await;
                        cdn::queued_or_active_crate_invalidations(&mut conn).await
                    })
                    .unwrap()
            };

            assert!(fetch_invalidations().is_empty());

            queue.process_next_crate(|krate| {
                assert_eq!("will_succeed", krate.name);
                Ok(BuildPackageSummary::default())
            })?;

            let queued_invalidations = fetch_invalidations();
            assert_eq!(queued_invalidations.len(), 3);
            assert!(
                queued_invalidations
                    .iter()
                    .all(|i| i.krate == "will_succeed")
            );

            queue.process_next_crate(|krate| {
                assert_eq!("will_fail", krate.name);
                anyhow::bail!("simulate a failure");
            })?;

            let queued_invalidations = fetch_invalidations();
            assert_eq!(queued_invalidations.len(), 6);
            assert!(
                queued_invalidations
                    .iter()
                    .skip(3)
                    .all(|i| i.krate == "will_fail")
            );

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
            env.runtime().block_on(async {
                let mut conn = env.async_db().await.async_conn().await;
                set_config(&mut conn, ConfigName::LastSeenIndexReference, "invalid")
                    .await
                    .unwrap();
            });

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

    #[test]
    fn test_add_long_name() {
        crate::test::wrapper(|env| {
            let queue = env.build_queue();

            let name: String = "krate".repeat(100);

            queue.add_crate(&name, "0.0.1", 0, None)?;

            queue.process_next_crate(|krate| {
                assert_eq!(name, krate.name);
                Ok(BuildPackageSummary::default())
            })?;

            Ok(())
        })
    }

    #[test]
    fn test_add_long_version() {
        crate::test::wrapper(|env| {
            let queue = env.build_queue();

            let version: String = "version".repeat(100);

            queue.add_crate("krate", &version, 0, None)?;

            queue.process_next_crate(|krate| {
                assert_eq!(version, krate.version);
                Ok(BuildPackageSummary::default())
            })?;

            Ok(())
        })
    }
}
