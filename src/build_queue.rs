use crate::db::AsyncPoolClient;
use crate::{
    BuildPackageSummary, Config, Context, Index, RustwideBuilder,
    cdn::{self, CdnMetrics},
    db::{
        CrateId, Pool, delete_crate, delete_version,
        types::{krate_name::KrateName, version::Version},
        update_latest_version_id,
    },
    docbuilder::{BuilderMetrics, PackageKind},
    error::Result,
    metrics::otel::AnyMeterProvider,
    storage::AsyncStorage,
    utils::{ConfigName, get_config, get_crate_priority, report_error, retry, set_config},
};
use anyhow::Context as _;
use chrono::NaiveDate;
use crates_index_diff::{Change, CrateVersion};
use fn_error_context::context;
use futures_util::{StreamExt, stream::TryStreamExt};
use opentelemetry::metrics::Counter;
use sqlx::Connection as _;
use std::{collections::HashMap, sync::Arc, time::Instant};
use tokio::runtime;
use tracing::{debug, error, info, instrument, warn};

#[derive(Debug)]
struct BuildQueueMetrics {
    queued_builds: Counter<u64>,
}

impl BuildQueueMetrics {
    fn new(meter_provider: &AnyMeterProvider) -> Self {
        let meter = meter_provider.meter("build_queue");
        const PREFIX: &str = "docsrs.build_queue";
        Self {
            queued_builds: meter
                .u64_counter(format!("{PREFIX}.queued_builds"))
                .with_unit("1")
                .build(),
        }
    }
}

pub(crate) const PRIORITY_DEFAULT: i32 = 0;
/// Used for workspaces to avoid blocking the queue (done through the cratesfyi CLI, not used in code)
#[allow(dead_code)]
pub(crate) const PRIORITY_DEPRIORITIZED: i32 = 1;
/// Rebuilds triggered from crates.io, see issue #2442
pub(crate) const PRIORITY_MANUAL_FROM_CRATES_IO: i32 = 5;
/// Used for rebuilds queued through cratesfyi for crate versions failed due to a broken Rustdoc nightly version.
/// Note: a broken rustdoc version does not necessarily imply a failed build.
pub(crate) const PRIORITY_BROKEN_RUSTDOC: i32 = 10;
/// Used by the synchronize cratesfyi command when queueing builds that are in the crates.io index but not in the database.
pub(crate) const PRIORITY_CONSISTENCY_CHECK: i32 = 15;
/// The static priority for background rebuilds, used when queueing rebuilds, and when rendering them collapsed in the UI.
pub(crate) const PRIORITY_CONTINUOUS: i32 = 20;

#[derive(Debug, Clone, Eq, PartialEq, serde::Serialize)]
pub(crate) struct QueuedCrate {
    #[serde(skip)]
    id: i32,
    pub(crate) name: String,
    pub(crate) version: Version,
    pub(crate) priority: i32,
    pub(crate) registry: Option<String>,
    pub(crate) attempt: i32,
}

#[derive(Debug)]
pub struct AsyncBuildQueue {
    config: Arc<Config>,
    storage: Arc<AsyncStorage>,
    pub(crate) db: Pool,
    queue_metrics: BuildQueueMetrics,
    builder_metrics: Arc<BuilderMetrics>,
    cdn_metrics: Arc<CdnMetrics>,
    max_attempts: i32,
}

impl AsyncBuildQueue {
    pub fn new(
        db: Pool,
        config: Arc<Config>,
        storage: Arc<AsyncStorage>,
        cdn_metrics: Arc<CdnMetrics>,
        otel_meter_provider: &AnyMeterProvider,
    ) -> Self {
        AsyncBuildQueue {
            max_attempts: config.build_attempts.into(),
            config,
            db,
            storage,
            queue_metrics: BuildQueueMetrics::new(otel_meter_provider),
            builder_metrics: Arc::new(BuilderMetrics::new(otel_meter_provider)),
            cdn_metrics,
        }
    }

    pub fn builder_metrics(&self) -> Arc<BuilderMetrics> {
        self.builder_metrics.clone()
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
        version: &Version,
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
            version as _,
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
            r#"SELECT
                id,
                name,
                version as "version: Version",
                priority,
                registry,
                attempt
             FROM queue
             WHERE attempt < $1
             ORDER BY priority ASC, attempt ASC, id ASC"#,
            self.max_attempts
        )
        .fetch_all(&mut *conn)
        .await?)
    }

    pub(crate) async fn has_build_queued(&self, name: &str, version: &Version) -> Result<bool> {
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
            version as _,
        )
        .fetch_optional(&mut *conn)
        .await?
        .is_some())
    }

    async fn remove_crate_from_queue(&self, name: &str) -> Result<()> {
        let mut conn = self.db.get_async().await?;
        sqlx::query!(
            "DELETE
             FROM queue
             WHERE name = $1
             ",
            name
        )
        .execute(&mut *conn)
        .await?;

        Ok(())
    }

    async fn remove_version_from_queue(&self, name: &str, version: &Version) -> Result<()> {
        let mut conn = self.db.get_async().await?;
        sqlx::query!(
            "DELETE
             FROM queue
             WHERE
                name = $1 AND
                version = $2
             ",
            name,
            version as _,
        )
        .execute(&mut *conn)
        .await?;

        Ok(())
    }

    /// Decreases the priority of all releases currently present in the queue not matching the version passed to *at least* new_priority.
    pub(crate) async fn deprioritize_other_releases(
        &self,
        name: &str,
        latest_version: &Version,
        new_priority: i32,
    ) -> Result<()> {
        let mut conn = self.db.get_async().await?;
        sqlx::query!(
            "UPDATE queue
             SET priority = GREATEST(priority, $1)
             WHERE
                name = $2
                AND version != $3
                AND attempt < $4
             ",
            new_priority,
            name,
            latest_version as _,
            self.max_attempts,
        )
        .execute(&mut *conn)
        .await?;

        Ok(())
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
    async fn queue_crate_invalidation(&self, krate: &str) {
        let krate = match krate
            .parse::<KrateName>()
            .with_context(|| format!("can't parse crate name '{}'", krate))
        {
            Ok(krate) => krate,
            Err(err) => {
                report_error(&err);
                return;
            }
        };

        if let Err(err) =
            cdn::queue_crate_invalidation(&self.config, &self.cdn_metrics, &krate).await
        {
            report_error(&err);
        }
    }

    /// Updates registry index repository and adds new crates into build queue.
    ///
    /// Returns the number of crates added
    pub async fn get_new_crates(&self, index: &Index) -> Result<usize> {
        let last_seen_reference = self.last_seen_reference().await?;
        let last_seen_reference = if let Some(oid) = last_seen_reference {
            oid
        } else {
            warn!(
                        "no last-seen reference found in our database. We assume a fresh install and
                         set the latest reference (HEAD) as last. This means we will then start to queue
                         builds for new releases only from now on, and not for all existing releases."
                    );
            index.latest_commit_reference().await?
        };

        index.set_last_seen_reference(last_seen_reference).await?;

        let (changes, new_reference) = index.peek_changes_ordered().await?;

        let mut conn = self.db.get_async().await?;

        debug!(last_seen_reference=%last_seen_reference, new_reference=%new_reference, "queueing changes");

        let crates_added = self
            .process_changes(&mut conn, &changes, index.repository_url())
            .await;

        // set the reference in the database
        // so this survives recreating the registry watcher
        // server.
        self.set_last_seen_reference(new_reference).await?;

        Ok(crates_added)
    }

    async fn process_changes(
        &self,
        conn: &mut AsyncPoolClient,
        changes: &Vec<Change>,
        registry: Option<&str>,
    ) -> usize {
        let mut crates_added = 0;

        for change in changes {
            match self.process_change(conn, change, registry).await {
                Ok(added) => {
                    if added {
                        crates_added += 1;
                    }
                }
                Err(err) => report_error(&err),
            }
        }
        crates_added
    }

    /// Process a crate change, returning whether the change was a crate addition or not.
    async fn process_change(
        &self,
        conn: &mut AsyncPoolClient,
        change: &Change,
        registry: Option<&str>,
    ) -> Result<bool> {
        match change {
            Change::Added(release) => self.process_version_added(conn, release, registry).await?,
            Change::AddedAndYanked(release) => {
                self.process_version_added(conn, release, registry).await?;
                self.process_version_yank_status(conn, release).await?;
            }
            Change::Unyanked(release) | Change::Yanked(release) => {
                self.process_version_yank_status(conn, release).await?
            }
            Change::CrateDeleted { name, .. } => {
                self.process_crate_deleted(conn, name.as_str()).await?
            }
            Change::VersionDeleted(release) => self.process_version_deleted(conn, release).await?,
        };
        Ok(change.added().is_some())
    }

    /// Processes crate changes, whether they got yanked or unyanked.
    async fn process_version_yank_status(
        &self,
        conn: &mut AsyncPoolClient,
        release: &CrateVersion,
    ) -> Result<()> {
        // FIXME: delay yanks of crates that have not yet finished building
        // https://github.com/rust-lang/docs.rs/issues/1934
        if let Ok(release_version) = Version::parse(&release.version) {
            self.set_yanked_inner(
                conn,
                release.name.as_str(),
                &release_version,
                release.yanked,
            )
            .await?;
        }

        self.queue_crate_invalidation(&release.name).await;
        Ok(())
    }

    async fn process_version_added(
        &self,
        conn: &mut AsyncPoolClient,
        release: &CrateVersion,
        registry: Option<&str>,
    ) -> Result<()> {
        let priority = get_crate_priority(conn, &release.name).await?;
        let version = &release
            .version
            .parse()
            .context("couldn't parse release version as semver")?;
        self.add_crate(&release.name, version, priority, registry)
            .await
            .with_context(|| {
                format!(
                    "failed adding {}-{} into build queue",
                    release.name, release.version
                )
            })?;
        debug!(
            name=%release.name,
            version=%release.version,
            "added into build queue",
        );
        self.queue_metrics.queued_builds.add(1, &[]);
        self.deprioritize_other_releases(&release.name, version, PRIORITY_MANUAL_FROM_CRATES_IO)
            .await
            .unwrap_or_else(|err| report_error(&err));
        Ok(())
    }

    async fn process_version_deleted(
        &self,
        conn: &mut AsyncPoolClient,
        release: &CrateVersion,
    ) -> Result<()> {
        let version: Version = release
            .version
            .parse()
            .context("couldn't parse release version as semver")?;

        delete_version(conn, &self.storage, &self.config, &release.name, &version)
            .await
            .with_context(|| {
                format!(
                    "failed to delete version {}-{}",
                    release.name, release.version
                )
            })?;
        info!(
            name=%release.name,
            version=%release.version,
            "release was deleted from the index and the database",
        );
        self.queue_crate_invalidation(&release.name).await;
        self.remove_version_from_queue(&release.name, &version)
            .await?;
        Ok(())
    }

    async fn process_crate_deleted(&self, conn: &mut AsyncPoolClient, krate: &str) -> Result<()> {
        delete_crate(conn, &self.storage, &self.config, krate)
            .await
            .with_context(|| format!("failed to delete crate {krate}"))?;
        info!(
            name=%krate,
            "crate deleted from the index and the database",
        );
        self.queue_crate_invalidation(krate).await;
        self.remove_crate_from_queue(krate).await
    }

    pub async fn set_yanked(&self, name: &str, version: &Version, yanked: bool) -> Result<()> {
        let mut conn = self.db.get_async().await?;
        self.set_yanked_inner(&mut conn, name, version, yanked)
            .await
    }

    #[context("error trying to set {name}-{version} to yanked: {yanked}")]
    async fn set_yanked_inner(
        &self,
        conn: &mut sqlx::PgConnection,
        name: &str,
        version: &Version,
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
            version as _,
            yanked,
        )
        .fetch_optional(&mut *conn)
        .await?
        {
            debug!(
                %name,
                %version,
                %activity,
                "updating latest version id"
            );
            update_latest_version_id(&mut *conn, crate_id).await?;
        } else {
            match self
                .has_build_queued(name, version)
                .await
                .context("error trying to fetch build queue")
            {
                Ok(false) => {
                    error!(
                        %name,
                        %version,
                        "tried to yank or unyank non-existing release",
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
    runtime: runtime::Handle,
    inner: Arc<AsyncBuildQueue>,
}

/// sync versions of async methods
impl BuildQueue {
    pub fn add_crate(
        &self,
        name: &str,
        version: &Version,
        priority: i32,
        registry: Option<&str>,
    ) -> Result<()> {
        self.runtime
            .block_on(self.inner.add_crate(name, version, priority, registry))
    }

    pub fn set_yanked(&self, name: &str, version: &Version, yanked: bool) -> Result<()> {
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
    pub fn new(runtime: runtime::Handle, inner: Arc<AsyncBuildQueue>) -> Self {
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
                r#"SELECT
                    id,
                    name,
                    version as "version: Version",
                    priority,
                    registry,
                    attempt
                 FROM queue
                 WHERE
                    attempt < $1 AND
                    (last_attempt IS NULL OR last_attempt < NOW() - make_interval(secs => $2))
                 ORDER BY priority ASC, attempt ASC, id ASC
                 LIMIT 1
                 FOR UPDATE SKIP LOCKED"#,
                self.inner.max_attempts,
                self.inner.config.delay_between_build_attempts.as_secs_f64(),
            )
            .fetch_optional(&mut *transaction),
        )? {
            Some(krate) => krate,
            None => return Ok(()),
        };

        let res = {
            let instant = Instant::now();
            let res = f(&to_process);
            let elapsed = instant.elapsed().as_secs_f64();
            self.inner.builder_metrics.build_time.record(elapsed, &[]);
            res
        };

        self.inner.builder_metrics.total_builds.add(1, &[]);

        self.runtime
            .block_on(self.inner.queue_crate_invalidation(&to_process.name));

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
                self.inner.builder_metrics.failed_builds.add(1, &[]);
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

    /// Builds the top package from the queue. Returns whether there was a package in the queue.
    ///
    /// Note that this will return `Ok(true)` even if the package failed to build.
    pub(crate) fn build_next_queue_package(
        &self,
        context: &Context,
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

            if let Err(err) = builder
                .update_toolchain_and_add_essential_files()
                .context("Updating toolchain failed, locking queue")
            {
                report_error(&err);
                self.lock()?;
                return Err(err);
            }

            builder.build_package(&krate.name, &krate.version, kind, krate.attempt == 0)
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
        .filter_map(|(priority, count)| (*priority >= PRIORITY_CONTINUOUS).then_some(count))
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
        r#"SELECT i.* FROM (
             SELECT
                 c.name,
                 r.version as "version: Version",
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
         LIMIT $1"#,
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
                .add_crate(&row.name, &row.version, PRIORITY_CONTINUOUS, None)
                .await?;
        }
    }

    Ok(())
}

/// Queue rebuilds for failed crates due to a faulty version of rustdoc
///
/// It is assumed that the version of rustdoc matches the one of rustc, which is persisted in the DB.
/// The priority of the resulting rebuild requests will be lower than previously failed builds.
/// If a crate is already queued to be rebuilt, it will not be requeued.
/// Start date is inclusive, end date is exclusive.
#[instrument(skip_all)]
pub async fn queue_rebuilds_faulty_rustdoc(
    conn: &mut sqlx::PgConnection,
    build_queue: &AsyncBuildQueue,
    start_nightly_date: &NaiveDate,
    end_nightly_date: &Option<NaiveDate>,
) -> Result<i32> {
    let end_nightly_date =
        end_nightly_date.unwrap_or_else(|| start_nightly_date.succ_opt().unwrap());
    let mut results = sqlx::query!(
        r#"
SELECT c.name,
       r.version AS "version: Version"
FROM crates AS c
         JOIN releases AS r
              ON c.id = r.crate_id
         JOIN release_build_status AS rbs
            ON rbs.rid = r.id
         JOIN builds AS b
             ON b.rid = r.id
             AND b.build_finished = rbs.last_build_time
             AND b.rustc_nightly_date >= $1
             AND b.rustc_nightly_date < $2


"#,
        start_nightly_date,
        end_nightly_date
    )
    .fetch(&mut *conn);

    let mut results_count = 0;
    while let Some(row) = results.next().await {
        let row = row?;

        if !build_queue
            .has_build_queued(&row.name, &row.version)
            .await?
        {
            results_count += 1;
            info!(
                name=%row.name,
                version=%row.version,
                priority=PRIORITY_BROKEN_RUSTDOC,
               "queueing rebuild"
            );
            build_queue
                .add_crate(&row.name, &row.version, PRIORITY_BROKEN_RUSTDOC, None)
                .await?;
        }
    }

    Ok(results_count)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::types::BuildStatus;
    use crate::test::{FakeBuild, KRATE, TestEnvironment, V1, V2};
    use chrono::Utc;
    use std::time::Duration;

    #[tokio::test(flavor = "multi_thread")]
    async fn test_process_version_added() -> Result<()> {
        let env = TestEnvironment::new().await?;
        let build_queue = env.async_build_queue();
        let mut conn = env.async_db().async_conn().await;

        let krate = CrateVersion {
            name: "krate".parse()?,
            version: V1.to_string().parse()?,
            ..Default::default()
        };
        build_queue
            .process_version_added(&mut conn, &krate, None)
            .await?;
        let queue = build_queue.queued_crates().await?;
        assert_eq!(queue.len(), 1);
        assert_eq!(queue[0].priority, PRIORITY_DEFAULT);

        let krate = CrateVersion {
            name: "krate".parse()?,
            version: V2.to_string().parse()?,
            ..Default::default()
        };
        build_queue
            .process_version_added(&mut conn, &krate, None)
            .await?;
        let queue = build_queue.queued_crates().await?;
        assert_eq!(queue.len(), 2);
        // The other queued version should be deprioritized
        assert_eq!(queue[0].version, V2);
        assert_eq!(queue[0].priority, PRIORITY_DEFAULT);
        assert_eq!(queue[1].version, V1);
        assert_eq!(queue[1].priority, PRIORITY_MANUAL_FROM_CRATES_IO);

        Ok(())
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_process_version_yank_status() -> Result<()> {
        let env = TestEnvironment::new().await?;
        let build_queue = env.async_build_queue();
        let mut conn = env.async_db().async_conn().await;

        // Given a release that is yanked
        let id = env
            .fake_release()
            .await
            .name("krate")
            .version(V1)
            .create()
            .await?;
        // Simulate a yank change
        let krate = CrateVersion {
            name: "krate".parse()?,
            version: V1.to_string().parse()?,
            yanked: true,
            ..Default::default()
        };
        build_queue
            .process_version_yank_status(&mut conn, &krate)
            .await?;

        // And verify it's actually marked as yanked
        let row = sqlx::query!(
            "SELECT yanked
             FROM releases
             WHERE id = $1",
            id.0
        )
        .fetch_one(&mut *conn)
        .await?;
        assert_eq!(row.yanked, Some(true));

        // Verify whether we can unyank it too
        let krate = CrateVersion {
            name: "krate".parse()?,
            version: V1.to_string().parse()?,
            yanked: false,
            ..Default::default()
        };
        build_queue
            .process_version_yank_status(&mut conn, &krate)
            .await?;

        let row = sqlx::query!(
            "SELECT yanked
             FROM releases
             WHERE id = $1",
            id.0
        )
        .fetch_one(&mut *conn)
        .await?;
        assert_eq!(row.yanked, Some(false));

        Ok(())
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_process_crate_deleted() -> Result<()> {
        let env = TestEnvironment::new().await?;
        let build_queue = env.async_build_queue();
        let mut conn = env.async_db().async_conn().await;

        env.fake_release()
            .await
            .name("krate")
            .version(V1)
            .create()
            .await?;
        build_queue
            .process_crate_deleted(&mut conn, "krate")
            .await?;

        let row = sqlx::query!(
            "SELECT id
             FROM crates
             WHERE name = $1",
            "krate"
        )
        .fetch_optional(&mut *conn)
        .await?;
        assert!(row.is_none());
        Ok(())
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_process_version_deleted() -> Result<()> {
        let env = TestEnvironment::new().await?;
        let build_queue = env.async_build_queue();
        let mut conn = env.async_db().async_conn().await;

        let rid_1 = env
            .fake_release()
            .await
            .name("krate")
            .version(V1)
            .create()
            .await?;
        env.fake_release()
            .await
            .name("krate")
            .version(V2)
            .create()
            .await?;

        let krate = CrateVersion {
            name: "krate".parse()?,
            version: V2.to_string().parse()?,
            ..Default::default()
        };
        build_queue
            .process_version_deleted(&mut conn, &krate)
            .await?;

        let row = sqlx::query!(
            "SELECT id
             FROM releases",
        )
        .fetch_all(&mut *conn)
        .await?;
        assert_eq!(row.len(), 1);
        assert_eq!(row[0].id, rid_1.0);
        Ok(())
    }

    /// Ensure changes can be processed with graceful error handling and proper tracking of added versions
    #[tokio::test(flavor = "multi_thread")]
    async fn test_process_changes() -> Result<()> {
        let env = TestEnvironment::new().await?;
        let build_queue = env.async_build_queue();
        let mut conn = env.async_db().async_conn().await;

        env.fake_release()
            .await
            .name("krate_already_present")
            .version(V1)
            .create()
            .await?;

        let krate1 = CrateVersion {
            name: "krate1".parse()?,
            version: V1.to_string().parse()?,
            ..Default::default()
        };
        let krate2 = CrateVersion {
            name: "krate2".parse()?,
            version: V1.to_string().parse()?,
            ..Default::default()
        };
        let krate_already_present = CrateVersion {
            name: "krate_already_present".parse()?,
            version: V1.to_string().parse()?,
            ..Default::default()
        };
        let non_existing_krate = CrateVersion {
            name: "krate_already_present".parse()?,
            version: V2.to_string().parse()?,
            ..Default::default()
        };
        let added = build_queue
            .process_changes(
                &mut conn,
                &vec![
                    Change::Added(krate1),                         // Should be added correctly
                    Change::Added(krate2),                         // Should be added correctly
                    Change::VersionDeleted(krate_already_present), // Should be deleted correctly, without affecting the returned counter
                    Change::VersionDeleted(non_existing_krate), // Should error out, but the error should be handled gracefully
                ],
                None,
            )
            .await;

        assert_eq!(added, 2);
        Ok(())
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_rebuild_when_old() -> Result<()> {
        let env = TestEnvironment::with_config(
            TestEnvironment::base_config()
                .max_queued_rebuilds(Some(100))
                .build()?,
        )
        .await?;

        env.fake_release()
            .await
            .name("foo")
            .version(V1)
            .builds(vec![
                FakeBuild::default().rustc_version("rustc 1.84.0-nightly (e7c0d2750 2020-10-15)"),
            ])
            .create()
            .await?;

        let build_queue = env.async_build_queue();
        assert!(build_queue.queued_crates().await?.is_empty());

        let mut conn = env.async_db().async_conn().await;
        queue_rebuilds(&mut conn, env.config(), build_queue).await?;

        let queue = build_queue.queued_crates().await?;
        assert_eq!(queue.len(), 1);
        assert_eq!(queue[0].name, "foo");
        assert_eq!(queue[0].version, V1);
        assert_eq!(queue[0].priority, PRIORITY_CONTINUOUS);

        Ok(())
    }

    /// Verifies whether a rebuild is queued for all releases with the latest build performed with a specific nightly version of rustdoc
    #[tokio::test(flavor = "multi_thread")]
    async fn test_rebuild_broken_rustdoc_specific_date_simple() -> Result<()> {
        let env = TestEnvironment::new().await?;

        // Matrix of test builds (crate name, nightly date, version)
        let build_matrix = [
            // Should be skipped since this is not the latest build for this release
            ("foo1", NaiveDate::from_ymd_opt(2020, 10, 1).unwrap(), V1),
            // All those should match
            ("foo1", NaiveDate::from_ymd_opt(2020, 10, 2).unwrap(), V1),
            ("foo1", NaiveDate::from_ymd_opt(2020, 10, 2).unwrap(), V2),
            ("foo2", NaiveDate::from_ymd_opt(2020, 10, 2).unwrap(), V1),
            // Should be skipped since the nightly doesn't match
            ("foo2", NaiveDate::from_ymd_opt(2020, 10, 3).unwrap(), V2),
        ];
        for build in build_matrix.into_iter() {
            let (crate_name, nightly, version) = build;
            env.fake_release()
                .await
                .name(crate_name)
                .version(version)
                .builds(vec![
                    FakeBuild::default()
                        .rustc_version(
                            format!(
                                "rustc 1.84.0-nightly (e7c0d2750 {})",
                                nightly.format("%Y-%m-%d")
                            )
                            .as_str(),
                        )
                        .build_status(BuildStatus::Failure),
                ])
                .create()
                .await?;
        }

        let build_queue = env.async_build_queue();
        assert!(build_queue.queued_crates().await?.is_empty());

        let mut conn = env.async_db().async_conn().await;
        queue_rebuilds_faulty_rustdoc(
            &mut conn,
            build_queue,
            &NaiveDate::from_ymd_opt(2020, 10, 2).unwrap(),
            &None,
        )
        .await?;

        let queue = build_queue.queued_crates().await?;
        assert_eq!(queue.len(), 3);
        assert_eq!(queue[0].name, "foo1");
        assert_eq!(queue[0].version, V1);
        assert_eq!(queue[0].priority, PRIORITY_BROKEN_RUSTDOC);
        assert_eq!(queue[1].name, "foo1");
        assert_eq!(queue[1].version, V2);
        assert_eq!(queue[1].priority, PRIORITY_BROKEN_RUSTDOC);
        assert_eq!(queue[2].name, "foo2");
        assert_eq!(queue[2].version, V1);
        assert_eq!(queue[2].priority, PRIORITY_BROKEN_RUSTDOC);

        Ok(())
    }

    /// Verifies whether a rebuild is NOT queued for any crate if the nightly specified doesn't match any latest build of any release
    #[tokio::test(flavor = "multi_thread")]
    async fn test_rebuild_broken_rustdoc_specific_date_skipped() -> Result<()> {
        let env = TestEnvironment::new().await?;

        // Matrix of test builds (crate name, nightly date, version)
        let build_matrix = [
            // Should be skipped since this is not the latest build for this release even if the nightly matches
            ("foo1", NaiveDate::from_ymd_opt(2020, 10, 3).unwrap(), V1),
            // Should be skipped since the nightly doesn't match
            ("foo1", NaiveDate::from_ymd_opt(2020, 10, 2).unwrap(), V1),
            // Should be skipped since the nightly doesn't match
            ("foo2", NaiveDate::from_ymd_opt(2020, 10, 4).unwrap(), V1),
        ];
        for build in build_matrix.into_iter() {
            let (crate_name, nightly, version) = build;
            env.fake_release()
                .await
                .name(crate_name)
                .version(version)
                .builds(vec![
                    FakeBuild::default()
                        .rustc_version(
                            format!(
                                "rustc 1.84.0-nightly (e7c0d2750 {})",
                                nightly.format("%Y-%m-%d")
                            )
                            .as_str(),
                        )
                        .build_status(BuildStatus::Failure),
                ])
                .create()
                .await?;
        }

        let build_queue = env.async_build_queue();
        assert!(build_queue.queued_crates().await?.is_empty());

        let mut conn = env.async_db().async_conn().await;
        queue_rebuilds_faulty_rustdoc(
            &mut conn,
            build_queue,
            &NaiveDate::from_ymd_opt(2020, 10, 3).unwrap(),
            &None,
        )
        .await?;

        let queue = build_queue.queued_crates().await?;
        assert_eq!(queue.len(), 0);

        Ok(())
    }

    /// Verifies whether a rebuild is queued for all releases with the latest build performed with a nightly version between two dates
    #[tokio::test(flavor = "multi_thread")]
    async fn test_rebuild_broken_rustdoc_date_range() -> Result<()> {
        let env = TestEnvironment::new().await?;

        // Matrix of test builds (crate name, nightly date, version)
        let build_matrix = [
            // Should be skipped since this is not the latest build for this release
            ("foo1", NaiveDate::from_ymd_opt(2020, 10, 1).unwrap(), V1),
            // All those should match
            ("foo1", NaiveDate::from_ymd_opt(2020, 10, 2).unwrap(), V1),
            ("foo1", NaiveDate::from_ymd_opt(2020, 10, 3).unwrap(), V2),
            ("foo2", NaiveDate::from_ymd_opt(2020, 10, 4).unwrap(), V1),
            // Should be skipped since the nightly doesn't match (end date is exclusive)
            ("foo2", NaiveDate::from_ymd_opt(2020, 10, 5).unwrap(), V2),
        ];
        for build in build_matrix.into_iter() {
            let (crate_name, nightly, version) = build;
            env.fake_release()
                .await
                .name(crate_name)
                .version(version)
                .builds(vec![
                    FakeBuild::default()
                        .rustc_version(
                            format!(
                                "rustc 1.84.0-nightly (e7c0d2750 {})",
                                nightly.format("%Y-%m-%d")
                            )
                            .as_str(),
                        )
                        .build_status(BuildStatus::Failure),
                ])
                .create()
                .await?;
        }

        let build_queue = env.async_build_queue();
        assert!(build_queue.queued_crates().await?.is_empty());

        let mut conn = env.async_db().async_conn().await;
        queue_rebuilds_faulty_rustdoc(
            &mut conn,
            build_queue,
            &NaiveDate::from_ymd_opt(2020, 10, 2).unwrap(),
            &NaiveDate::from_ymd_opt(2020, 10, 5),
        )
        .await?;

        let queue = build_queue.queued_crates().await?;
        assert_eq!(queue.len(), 3);
        assert_eq!(queue[0].name, "foo1");
        assert_eq!(queue[0].version, V1);
        assert_eq!(queue[0].priority, PRIORITY_BROKEN_RUSTDOC);
        assert_eq!(queue[1].name, "foo1");
        assert_eq!(queue[1].version, V2);
        assert_eq!(queue[1].priority, PRIORITY_BROKEN_RUSTDOC);
        assert_eq!(queue[2].name, "foo2");
        assert_eq!(queue[2].version, V1);
        assert_eq!(queue[2].priority, PRIORITY_BROKEN_RUSTDOC);

        Ok(())
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_still_rebuild_when_full_with_failed() -> Result<()> {
        let env = TestEnvironment::with_config(
            TestEnvironment::base_config()
                .max_queued_rebuilds(Some(1))
                .build()?,
        )
        .await?;

        let build_queue = env.async_build_queue();
        build_queue
            .add_crate("foo1", &V1, PRIORITY_CONTINUOUS, None)
            .await?;
        build_queue
            .add_crate("foo2", &V1, PRIORITY_CONTINUOUS, None)
            .await?;

        let mut conn = env.async_db().async_conn().await;
        sqlx::query!("UPDATE queue SET attempt = 99")
            .execute(&mut *conn)
            .await?;

        assert_eq!(build_queue.queued_crates().await?.len(), 0);

        env.fake_release()
            .await
            .name("foo")
            .version(V1)
            .builds(vec![
                FakeBuild::default().rustc_version("rustc 1.84.0-nightly (e7c0d2750 2020-10-15)"),
            ])
            .create()
            .await?;

        let build_queue = env.async_build_queue();
        queue_rebuilds(&mut conn, env.config(), build_queue).await?;

        assert_eq!(build_queue.queued_crates().await?.len(), 1);

        Ok(())
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_dont_rebuild_when_full() -> Result<()> {
        let env = TestEnvironment::with_config(
            TestEnvironment::base_config()
                .max_queued_rebuilds(Some(1))
                .build()?,
        )
        .await?;

        let build_queue = env.async_build_queue();
        build_queue
            .add_crate("foo1", &V1, PRIORITY_CONTINUOUS, None)
            .await?;
        build_queue
            .add_crate("foo2", &V1, PRIORITY_CONTINUOUS, None)
            .await?;

        env.fake_release()
            .await
            .name("foo")
            .version(V1)
            .builds(vec![
                FakeBuild::default().rustc_version("rustc 1.84.0-nightly (e7c0d2750 2020-10-15)"),
            ])
            .create()
            .await?;

        let build_queue = env.async_build_queue();
        assert_eq!(build_queue.queued_crates().await?.len(), 2);

        let mut conn = env.async_db().async_conn().await;
        queue_rebuilds(&mut conn, env.config(), build_queue).await?;

        assert_eq!(build_queue.queued_crates().await?.len(), 2);

        Ok(())
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_add_duplicate_doesnt_fail_last_priority_wins() -> Result<()> {
        let env = TestEnvironment::new().await?;

        let queue = env.async_build_queue();

        queue.add_crate("some_crate", &V1, 0, None).await?;
        queue.add_crate("some_crate", &V1, 9, None).await?;

        let queued_crates = queue.queued_crates().await?;
        assert_eq!(queued_crates.len(), 1);
        assert_eq!(queued_crates[0].priority, 9);

        Ok(())
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_add_duplicate_resets_attempts_and_priority() -> Result<()> {
        let env =
            TestEnvironment::with_config(TestEnvironment::base_config().build_attempts(5).build()?)
                .await?;

        let queue = env.async_build_queue();

        let mut conn = env.async_db().async_conn().await;
        sqlx::query!(
            "
                INSERT INTO queue (name, version, priority, attempt, last_attempt )
                VALUES ('failed_crate', $1, 0, 99, NOW())",
            V1 as _
        )
        .execute(&mut *conn)
        .await?;

        assert_eq!(queue.pending_count().await?, 0);

        queue.add_crate("failed_crate", &V1, 9, None).await?;

        assert_eq!(queue.pending_count().await?, 1);

        let row = sqlx::query!(
            "SELECT priority, attempt, last_attempt
                     FROM queue
                     WHERE name = $1 AND version = $2",
            "failed_crate",
            V1 as _
        )
        .fetch_one(&mut *conn)
        .await?;

        assert_eq!(row.priority, 9);
        assert_eq!(row.attempt, 0);
        assert!(row.last_attempt.is_none());
        Ok(())
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_has_build_queued() -> Result<()> {
        let env = TestEnvironment::new().await?;

        let queue = env.async_build_queue();

        queue.add_crate("dummy", &V1, 0, None).await?;

        let mut conn = env.async_db().async_conn().await;
        assert!(queue.has_build_queued("dummy", &V1).await.unwrap());

        sqlx::query!("UPDATE queue SET attempt = 6")
            .execute(&mut *conn)
            .await
            .unwrap();

        assert!(!queue.has_build_queued("dummy", &V1).await.unwrap());

        Ok(())
    }

    #[test]
    fn test_wait_between_build_attempts() -> Result<()> {
        let env = TestEnvironment::with_config_and_runtime(
            TestEnvironment::base_config()
                .build_attempts(99)
                .delay_between_build_attempts(Duration::from_secs(1))
                .build()?,
        )?;

        let runtime = env.runtime();

        let queue = env.build_queue();

        queue.add_crate("krate", &V1, 0, None)?;

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
            let mut conn = env.async_db().async_conn().await;
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
    }

    #[test]
    fn test_add_and_process_crates() -> Result<()> {
        const MAX_ATTEMPTS: u16 = 3;
        let env = TestEnvironment::with_config_and_runtime(
            TestEnvironment::base_config()
                .build_attempts(MAX_ATTEMPTS)
                .delay_between_build_attempts(Duration::ZERO)
                .build()?,
        )?;

        let queue = env.build_queue();

        let test_crates = [
            ("low-priority", 1000),
            ("high-priority-foo", -1000),
            ("medium-priority", -10),
            ("high-priority-bar", -1000),
            ("standard-priority", 0),
            ("high-priority-baz", -1000),
        ];
        for krate in &test_crates {
            queue.add_crate(krate.0, &V1, krate.1, None)?;
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

        let collected_metrics = env.collected_metrics();

        assert_eq!(
            collected_metrics
                .get_metric("builder", "docsrs.builder.total_builds")?
                .get_u64_counter()
                .value(),
            9
        );

        assert_eq!(
            collected_metrics
                .get_metric("builder", "docsrs.builder.failed_builds")?
                .get_u64_counter()
                .value(),
            1
        );

        assert_eq!(
            dbg!(
                collected_metrics
                    .get_metric("builder", "docsrs.builder.build_time")?
                    .get_f64_histogram()
                    .count()
            ),
            9
        );

        Ok(())
    }

    #[test]
    fn test_invalidate_cdn_after_error() -> Result<()> {
        let mut fastly_api = mockito::Server::new();

        let env = TestEnvironment::with_config_and_runtime(
            TestEnvironment::base_config()
                .fastly_api_host(fastly_api.url().parse().unwrap())
                .fastly_api_token(Some("test-token".into()))
                .fastly_service_sid(Some("test-sid-1".into()))
                .build()?,
        )?;

        let queue = env.build_queue();

        let m = fastly_api
            .mock("POST", "/service/test-sid-1/purge")
            .with_status(200)
            .create();

        queue.add_crate("will_fail", &V1, 0, None)?;

        queue.process_next_crate(|krate| {
            assert_eq!("will_fail", krate.name);
            anyhow::bail!("simulate a failure");
        })?;

        m.expect(1).assert();

        Ok(())
    }
    #[test]
    fn test_invalidate_cdn_after_build() -> Result<()> {
        let mut fastly_api = mockito::Server::new();

        let env = TestEnvironment::with_config_and_runtime(
            TestEnvironment::base_config()
                .fastly_api_host(fastly_api.url().parse().unwrap())
                .fastly_api_token(Some("test-token".into()))
                .fastly_service_sid(Some("test-sid-1".into()))
                .build()?,
        )?;

        let queue = env.build_queue();

        let m = fastly_api
            .mock("POST", "/service/test-sid-1/purge")
            .with_status(200)
            .create();

        queue.add_crate("will_succeed", &V1, -1, None)?;

        queue.process_next_crate(|krate| {
            assert_eq!("will_succeed", krate.name);
            Ok(BuildPackageSummary::default())
        })?;

        m.expect(1).assert();

        Ok(())
    }

    #[test]
    fn test_pending_count() -> Result<()> {
        let env = TestEnvironment::new_with_runtime()?;

        let queue = env.build_queue();

        assert_eq!(queue.pending_count()?, 0);
        queue.add_crate("foo", &V1, 0, None)?;
        assert_eq!(queue.pending_count()?, 1);
        queue.add_crate("bar", &V1, 0, None)?;
        assert_eq!(queue.pending_count()?, 2);

        queue.process_next_crate(|krate| {
            assert_eq!("foo", krate.name);
            Ok(BuildPackageSummary::default())
        })?;
        assert_eq!(queue.pending_count()?, 1);

        drop(env);

        Ok(())
    }

    #[test]
    fn test_prioritized_count() -> Result<()> {
        let env = TestEnvironment::new_with_runtime()?;

        let queue = env.build_queue();

        assert_eq!(queue.prioritized_count()?, 0);
        queue.add_crate("foo", &V1, 0, None)?;
        assert_eq!(queue.prioritized_count()?, 1);
        queue.add_crate("bar", &V1, -100, None)?;
        assert_eq!(queue.prioritized_count()?, 2);
        queue.add_crate("baz", &V1, 100, None)?;
        assert_eq!(queue.prioritized_count()?, 2);

        queue.process_next_crate(|krate| {
            assert_eq!("bar", krate.name);
            Ok(BuildPackageSummary::default())
        })?;
        assert_eq!(queue.prioritized_count()?, 1);

        Ok(())
    }

    #[test]
    fn test_count_by_priority() -> Result<()> {
        let env = TestEnvironment::new_with_runtime()?;

        let queue = env.build_queue();

        assert!(queue.pending_count_by_priority()?.is_empty());

        queue.add_crate("one", &V1, 1, None)?;
        queue.add_crate("two", &V2, 2, None)?;
        queue.add_crate("two_more", &V2, 2, None)?;

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
        let env = TestEnvironment::with_config_and_runtime(
            TestEnvironment::base_config()
                .build_attempts(MAX_ATTEMPTS)
                .delay_between_build_attempts(Duration::ZERO)
                .build()?,
        )?;

        const MAX_ATTEMPTS: u16 = 3;

        let queue = env.build_queue();

        assert_eq!(queue.failed_count()?, 0);
        queue.add_crate("foo", &V1, -100, None)?;
        assert_eq!(queue.failed_count()?, 0);
        queue.add_crate("bar", &V1, 0, None)?;

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
    }

    #[test]
    fn test_failed_count_after_error() -> Result<()> {
        let env = TestEnvironment::with_config_and_runtime(
            TestEnvironment::base_config()
                .build_attempts(MAX_ATTEMPTS)
                .delay_between_build_attempts(Duration::ZERO)
                .build()?,
        )?;

        const MAX_ATTEMPTS: u16 = 3;

        let queue = env.build_queue();

        assert_eq!(queue.failed_count()?, 0);
        queue.add_crate("foo", &V1, -100, None)?;
        assert_eq!(queue.failed_count()?, 0);
        queue.add_crate("bar", &V1, 0, None)?;

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
    }

    #[test]
    fn test_queued_crates() -> Result<()> {
        let env = TestEnvironment::new_with_runtime()?;

        let queue = env.build_queue();

        let test_crates = [("bar", 0), ("foo", -10), ("baz", 10)];
        for krate in &test_crates {
            queue.add_crate(krate.0, &V1, krate.1, None)?;
        }

        assert_eq!(
            vec![
                ("foo".into(), V1, -10),
                ("bar".into(), V1, 0),
                ("baz".into(), V1, 10),
            ],
            queue
                .queued_crates()?
                .into_iter()
                .map(|c| (c.name.clone(), c.version, c.priority))
                .collect::<Vec<_>>()
        );

        Ok(())
    }

    #[test]
    fn test_last_seen_reference_in_db() -> Result<()> {
        let env = TestEnvironment::new_with_runtime()?;

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
    }

    #[test]
    fn test_broken_db_reference_breaks() -> Result<()> {
        let env = TestEnvironment::new_with_runtime()?;

        env.runtime().block_on(async {
            let mut conn = env.async_db().async_conn().await;
            set_config(&mut conn, ConfigName::LastSeenIndexReference, "invalid")
                .await
                .unwrap();
        });

        let queue = env.build_queue();
        assert!(queue.last_seen_reference().is_err());

        Ok(())
    }

    #[test]
    fn test_queue_lock() -> Result<()> {
        let env = TestEnvironment::new_with_runtime()?;

        let queue = env.build_queue();
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
        let env = TestEnvironment::new_with_runtime()?;

        let queue = env.build_queue();

        let name: String = "krate".repeat(100);

        queue.add_crate(&name, &V1, 0, None)?;

        queue.process_next_crate(|krate| {
            assert_eq!(name, krate.name);
            Ok(BuildPackageSummary::default())
        })?;

        Ok(())
    }

    #[test]
    fn test_add_long_version() -> Result<()> {
        let env = TestEnvironment::new_with_runtime()?;

        let queue = env.build_queue();

        let long_version = Version::parse(&format!(
            "1.2.3-{}+{}",
            "prerelease".repeat(100),
            "build".repeat(100)
        ))?;

        queue.add_crate("krate", &long_version, 0, None)?;

        queue.process_next_crate(|krate| {
            assert_eq!(long_version, krate.version);
            Ok(BuildPackageSummary::default())
        })?;

        Ok(())
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_delete_version_from_queue() -> Result<()> {
        let env = TestEnvironment::new().await?;

        let queue = env.async_build_queue();
        assert_eq!(queue.pending_count().await?, 0);

        queue.add_crate(KRATE, &V1, 0, None).await?;
        queue.add_crate(KRATE, &V2, 0, None).await?;

        assert_eq!(queue.pending_count().await?, 2);
        queue.remove_version_from_queue(KRATE, &V1).await?;

        assert_eq!(queue.pending_count().await?, 1);

        // only v2 remains
        if let [k] = queue.queued_crates().await?.as_slice() {
            assert_eq!(k.name, KRATE);
            assert_eq!(k.version, V2);
        } else {
            panic!("expected only one queued crate");
        }

        Ok(())
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_delete_crate_from_queue() -> Result<()> {
        let env = TestEnvironment::new().await?;

        let queue = env.async_build_queue();
        assert_eq!(queue.pending_count().await?, 0);

        queue.add_crate(KRATE, &V1, 0, None).await?;
        queue.add_crate(KRATE, &V2, 0, None).await?;

        assert_eq!(queue.pending_count().await?, 2);
        queue.remove_crate_from_queue(KRATE).await?;

        assert_eq!(queue.pending_count().await?, 0);

        Ok(())
    }
}
