//! Typed build handles and lifecycle transitions.
//!
//! A transition consumes its in-progress handle and checks the database state.

use crate::releases::update_build_status;
use anyhow::{Context as _, Result, anyhow};
use chrono::{DateTime, Utc};
use docs_rs_types::{
    BuildError, BuildId, BuildStatus, ByteSize, Duration, KrateName, ReleaseId, Version,
};
use docs_rs_utils::rustc_version::parse_rustc_date;
use futures_util::{StreamExt as _, TryStreamExt as _, stream};
use tracing::{debug, error};

#[derive(Debug, Copy, Clone, Default)]
pub enum BuildLogKind {
    #[default]
    Html,
    Json,
}

impl BuildLogKind {
    fn suffix(&self) -> &'static str {
        match self {
            Self::Html => "",
            Self::Json => "_json",
        }
    }
}

fn build_log_filename(target: &str, suffix: &str) -> String {
    format!("{target}{suffix}")
}

fn build_log_storage_path(build_id: BuildId, filename: &str) -> String {
    format!("build-logs/{build_id}/{filename}",)
}

/// A target log to upload and register as part of a batch.
#[derive(bon::Builder)]
pub struct NewBuildLog {
    #[builder(into)]
    target: String,
    #[builder(default)]
    kind: BuildLogKind,
    #[builder(into)]
    log: String,
    successful: bool,
}

impl NewBuildLog {
    fn filename(&self) -> String {
        build_log_filename(&self.target, self.kind.suffix())
    }

    fn storage_path(&self, build_id: BuildId) -> String {
        build_log_storage_path(
            build_id,
            &build_log_filename(&self.target, self.kind.suffix()),
        )
    }
}

/// A readable log descriptor. Content is loaded only by calling `fetch`.
#[derive(Debug)]
pub struct BuildLog {
    filename: String,
    successful: Option<bool>,
    storage_path: String,
}

impl BuildLog {
    pub fn storage(build_id: BuildId, filename: impl Into<String>, successful: bool) -> Self {
        let filename = filename.into();
        Self {
            storage_path: build_log_storage_path(build_id, &filename),
            filename,
            successful: Some(successful),
        }
    }

    pub fn filename(&self) -> &str {
        &self.filename
    }

    /// Unknown for older logs that have no registration record.
    pub fn successful(&self) -> Option<bool> {
        self.successful
    }

    pub async fn fetch(&self, storage: &docs_rs_storage::AsyncStorage) -> Result<String> {
        let path = &self.storage_path;
        let blob = Box::pin(storage.get(path, storage.config().max_file_size_for(path))).await?;
        String::from_utf8(blob.content).context("non utf8 build log")
    }
}

#[derive(Debug)]
pub struct InProgress;

#[derive(Debug, Clone, PartialEq)]
pub struct Finished {
    pub status: BuildStatus,
    pub errors: Option<String>,
    pub error_kind: Option<String>,
    /// Historical builds may lack completion metadata.
    pub finished_at: Option<DateTime<Utc>>,
    pub rustc_version: Option<String>,
    pub docsrs_version: Option<String>,
    pub memory_peak: Option<ByteSize>,
    pub documentation_size: Option<ByteSize>,
}

#[derive(Debug)]
pub struct EarlyFailure {
    pub errors: Option<String>,
    pub error_kind: Option<String>,
}

/// Error text and classification captured when configuring a transition.
#[derive(Debug, thiserror::Error)]
#[error("{message}")]
pub struct CompletionError {
    message: String,
    kind: &'static str,
}

impl CompletionError {
    pub fn new(error: &impl BuildError) -> Self {
        Self {
            message: error.to_string(),
            kind: error.kind(),
        }
    }
}

impl BuildError for CompletionError {
    fn kind(&self) -> &'static str {
        self.kind
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Build<State> {
    id: BuildId,
    started_at: Option<DateTime<Utc>>,
    logs: Vec<(String, bool)>,
    has_legacy_output: bool,
    default_target: Option<String>,
    state: State,
}

#[derive(sqlx::FromRow)]
struct BuildRow {
    id: BuildId,
    build_status: BuildStatus,
    build_started: Option<DateTime<Utc>>,
    build_finished: Option<DateTime<Utc>>,
    rustc_version: Option<String>,
    docsrs_version: Option<String>,
    memory_peak: Option<ByteSize>,
    documentation_size: Option<ByteSize>,
    errors: Option<String>,
    error_kind: Option<String>,
    has_legacy_output: bool,
    default_target: Option<String>,
    logs: Option<Vec<(String, bool)>>,
}

// All reads fetch log existence, never the potentially large legacy contents.
const READ_BUILDS: &str = r#"
    SELECT b.id, b.build_status, b.build_started, b.build_finished,
           b.rustc_version, b.docsrs_version, b.memory_peak, b.documentation_size,
           b.errors, b.error_kind, b.output IS NOT NULL AS has_legacy_output,
           r.default_target,
           (SELECT array_agg(row(l.log_filename, l.success) ORDER BY l.log_filename)
            FROM builds_logs l WHERE l.build_id = b.id) AS logs
    FROM builds b
    JOIN releases r ON r.id = b.rid
    JOIN crates c ON c.id = r.crate_id
    WHERE ($1::text IS NULL OR c.name = $1)
      AND ($2::text IS NULL OR r.version = $2)
      AND ($3::integer IS NULL OR b.id = $3)
    ORDER BY b.id DESC
"#;

impl BuildRow {
    fn into_build(self) -> Result<AnyBuild> {
        let logs = self.logs.unwrap_or_default();
        Ok(match (self.build_status, self.build_finished) {
            (BuildStatus::InProgress, None) => AnyBuild::InProgress(Build {
                id: self.id,
                started_at: self.build_started,
                logs,
                has_legacy_output: self.has_legacy_output,
                default_target: self.default_target,
                state: InProgress,
            }),
            (BuildStatus::Failure, None) => AnyBuild::EarlyFailure(Build {
                id: self.id,
                started_at: self.build_started,
                logs,
                has_legacy_output: self.has_legacy_output,
                default_target: self.default_target,
                state: EarlyFailure {
                    errors: self.errors,
                    error_kind: self.error_kind,
                },
            }),
            (status, finished_at) if status != BuildStatus::InProgress => {
                AnyBuild::Finished(Build {
                    id: self.id,
                    started_at: self.build_started,
                    logs,
                    has_legacy_output: self.has_legacy_output,
                    default_target: self.default_target,
                    state: Finished {
                        status,
                        errors: self.errors,
                        error_kind: self.error_kind,
                        finished_at,
                        rustc_version: self.rustc_version,
                        docsrs_version: self.docsrs_version,
                        memory_peak: self.memory_peak,
                        documentation_size: self.documentation_size,
                    },
                })
            }
            _ => {
                return Err(anyhow!(
                    "build {} has inconsistent lifecycle fields",
                    self.id
                ));
            }
        })
    }
}

impl AnyBuild {
    pub async fn fetch_legacy_output(&self, pool: &crate::Pool) -> Result<String> {
        match self {
            Self::InProgress(build) => build.fetch_legacy_output(pool).await,
            Self::Finished(build) => build.fetch_legacy_output(pool).await,
            Self::EarlyFailure(build) => build.fetch_legacy_output(pool).await,
        }
    }

    /// Start a new attempt, marking any previous in-progress attempts aborted.
    pub async fn start(
        conn: &mut sqlx::PgConnection,
        release_id: ReleaseId,
    ) -> Result<Build<InProgress>> {
        let mut transaction = sqlx::Connection::begin(conn).await?;
        let conn = &mut *transaction;
        let hostname = hostname::get()?;

        sqlx::query!(
            r#"UPDATE builds
           SET
               build_status = 'failure',
               errors = $1,
               build_finished = NOW()
           WHERE
               rid = $2
               AND build_status = 'in_progress'"#,
            "build aborted: builder process restarted before completion",
            release_id as _,
        )
        .execute(&mut *conn)
        .await?;

        let build_id = sqlx::query_scalar!(
            r#"INSERT INTO builds(rid, build_status, build_server, build_started)
         VALUES ($1, $2, $3, NOW())
         RETURNING id as "id: BuildId" "#,
            release_id.0,
            BuildStatus::InProgress as BuildStatus,
            hostname.to_str().unwrap_or(""),
        )
        .fetch_one(&mut *conn)
        .await?;

        update_build_status(conn, release_id).await?;

        let AnyBuild::InProgress(build) = AnyBuild::open(conn, build_id).await? else {
            unreachable!("new build is in progress");
        };
        transaction.commit().await?;
        Ok(build)
    }

    /// Open an existing attempt without creating a new one or changing its state.
    ///
    /// ```ignore
    /// match AnyBuild::open(conn, build_id).await? {
    ///     AnyBuild::InProgress(build) => {
    ///         let finished = build.finish()
    ///             .rustc_version("rustc 1.84.0-nightly (000000000 2024-10-01)")
    ///             .docsrs_version("docs.rs 1.0.0")
    ///             .successful(false)
    ///             .error(&error)
    ///             .save(conn)
    ///             .await?;
    ///     }
    ///     AnyBuild::Finished(_) | AnyBuild::EarlyFailure(_) => {}
    /// }
    /// ```
    pub async fn open(conn: &mut sqlx::PgConnection, id: BuildId) -> Result<AnyBuild> {
        sqlx::query_as::<_, BuildRow>(READ_BUILDS)
            .bind(None::<&str>)
            .bind(None::<&str>)
            .bind(id.0)
            .fetch_one(conn)
            .await?
            .into_build()
    }

    /// Load all attempts in one query, without loading legacy log contents.
    pub async fn for_release(
        conn: &mut sqlx::PgConnection,
        name: &KrateName,
        version: &Version,
    ) -> Result<Vec<AnyBuild>> {
        sqlx::query_as::<_, BuildRow>(READ_BUILDS)
            .bind(name.to_string())
            .bind(version.to_string())
            .bind(None::<i32>)
            .fetch_all(conn)
            .await?
            .into_iter()
            .map(BuildRow::into_build)
            .collect()
    }

    /// Load an attempt only if it belongs to the requested crate and version.
    pub async fn find_for_release(
        conn: &mut sqlx::PgConnection,
        name: &KrateName,
        version: &Version,
        id: BuildId,
    ) -> Result<Option<AnyBuild>> {
        sqlx::query_as::<_, BuildRow>(READ_BUILDS)
            .bind(name.to_string())
            .bind(version.to_string())
            .bind(id.0)
            .fetch_optional(conn)
            .await?
            .map(BuildRow::into_build)
            .transpose()
    }

    pub fn default_log_filename(&self) -> Option<String> {
        self.default_target()
            .map(|default_target| format!("{default_target}.txt"))
    }

    pub async fn list_build_logs(
        &self,
        storage: &docs_rs_storage::AsyncStorage,
    ) -> Result<Vec<BuildLog>> {
        match self {
            Self::InProgress(build) => build.list_build_logs(storage).await,
            Self::Finished(build) => build.list_build_logs(storage).await,
            Self::EarlyFailure(build) => build.list_build_logs(storage).await,
        }
    }

    pub fn build_log(&self, filename: &str) -> BuildLog {
        match self {
            Self::InProgress(build) => build.build_log(filename),
            Self::Finished(build) => build.build_log(filename),
            Self::EarlyFailure(build) => build.build_log(filename),
        }
    }

    pub fn errors(&self) -> Option<&str> {
        match self {
            Self::InProgress(_) => None,
            Self::Finished(build) => build.state.errors.as_deref(),
            Self::EarlyFailure(build) => build.state.errors.as_deref(),
        }
    }

    pub fn error_kind(&self) -> Option<&str> {
        match self {
            Self::InProgress(_) => None,
            Self::Finished(build) => build.state.error_kind.as_deref(),
            Self::EarlyFailure(build) => build.state.error_kind.as_deref(),
        }
    }

    pub fn id(&self) -> BuildId {
        match self {
            Self::InProgress(build) => build.id(),
            Self::Finished(build) => build.id(),
            Self::EarlyFailure(build) => build.id(),
        }
    }

    pub fn started_at(&self) -> Option<DateTime<Utc>> {
        match self {
            Self::InProgress(build) => build.started_at(),
            Self::Finished(build) => build.started_at(),
            Self::EarlyFailure(build) => build.started_at(),
        }
    }

    pub fn logs(&self) -> &[(String, bool)] {
        match self {
            Self::InProgress(build) => build.logs(),
            Self::Finished(build) => build.logs(),
            Self::EarlyFailure(build) => build.logs(),
        }
    }

    pub fn has_legacy_output(&self) -> bool {
        match self {
            Self::InProgress(build) => build.has_legacy_output(),
            Self::Finished(build) => build.has_legacy_output(),
            Self::EarlyFailure(build) => build.has_legacy_output(),
        }
    }

    pub fn default_target(&self) -> Option<&str> {
        match self {
            Self::InProgress(build) => build.default_target(),
            Self::Finished(build) => build.default_target(),
            Self::EarlyFailure(build) => build.default_target(),
        }
    }

    pub fn status(&self) -> BuildStatus {
        match self {
            Self::InProgress(_) => BuildStatus::InProgress,
            Self::EarlyFailure(_) => BuildStatus::Failure,
            Self::Finished(build) => build.state.status,
        }
    }

    /// Target failures only downgrade a successful build, never an existing failure.
    pub fn display_status(&self) -> BuildStatus {
        if self.status() == BuildStatus::Success && self.logs().iter().any(|(_, success)| !success)
        {
            BuildStatus::PartialFailure
        } else {
            self.status()
        }
    }

    pub fn build_time(&self) -> Option<DateTime<Utc>> {
        match self {
            Self::Finished(build) => build.build_time(),
            _ => self.started_at(),
        }
    }

    pub fn duration(&self, now: DateTime<Utc>) -> Option<Duration> {
        let end = match self {
            Self::InProgress(_) => now,
            Self::Finished(build) => build.state.finished_at?,
            Self::EarlyFailure(_) => return None,
        };
        (end - self.started_at()?).to_std().ok().map(Into::into)
    }
}

/// Loading a database row requires inspecting its state at runtime.
#[derive(Debug)]
pub enum AnyBuild {
    InProgress(Build<InProgress>),
    Finished(Build<Finished>),
    EarlyFailure(Build<EarlyFailure>),
}

impl<State> Build<State> {
    /// Fetch legacy database output lazily. Missing output is a not-found error.
    pub async fn fetch_legacy_output(&self, pool: &crate::Pool) -> Result<String> {
        let mut conn = pool.get_async().await?;
        sqlx::query_scalar::<_, Option<String>>("SELECT output FROM builds WHERE id = $1")
            .bind(self.id.0)
            .fetch_optional(&mut *conn)
            .await?
            .flatten()
            .ok_or_else(|| docs_rs_storage::PathNotFoundError.into())
    }

    /// List registered logs, falling back to storage for older attempts without
    /// log records. Legacy database output is fetched separately and has no files.
    pub async fn list_build_logs(
        &self,
        storage: &docs_rs_storage::AsyncStorage,
    ) -> Result<Vec<BuildLog>> {
        if self.has_legacy_output {
            return Ok(Vec::new());
        }
        if !self.logs.is_empty() {
            return Ok(self
                .logs
                .iter()
                .map(|(filename, _)| self.build_log(filename))
                .collect());
        }
        let prefix = format!("build-logs/{}/", self.id);
        storage
            .list_prefix(&prefix)
            .await
            .map_ok(|path| {
                self.build_log(
                    path.strip_prefix(&prefix)
                        .expect("storage lists only keys under the requested prefix"),
                )
            })
            .try_collect()
            .await
    }

    /// Resolve a log without fetching its content or checking storage existence.
    pub fn build_log(&self, filename: &str) -> BuildLog {
        BuildLog {
            filename: filename.to_owned(),
            successful: self
                .logs
                .iter()
                .find(|(name, _)| name == filename)
                .map(|(_, success)| *success),
            storage_path: build_log_storage_path(self.id, filename),
        }
    }

    pub fn started_at(&self) -> Option<DateTime<Utc>> {
        self.started_at
    }

    pub fn logs(&self) -> &[(String, bool)] {
        &self.logs
    }

    pub fn has_legacy_output(&self) -> bool {
        self.has_legacy_output
    }

    pub fn default_target(&self) -> Option<&str> {
        self.default_target.as_deref()
    }
    pub fn id(&self) -> BuildId {
        self.id
    }

    pub fn state(&self) -> &State {
        &self.state
    }
}

impl Build<Finished> {
    pub fn build_time(&self) -> Option<DateTime<Utc>> {
        self.state.finished_at.or(self.started_at)
    }
}

#[bon::bon]
impl Build<InProgress> {
    /// Upload a target log, then register it in the database. Missing logs
    /// are reported and omitted; failed uploads never create a log record.
    #[builder(finish_fn = save)]
    pub async fn publish_build_log(
        &mut self,
        #[builder(finish_fn)] conn: &mut sqlx::PgConnection,
        #[builder(finish_fn)] storage: &docs_rs_storage::AsyncStorage,
        #[builder(into)] target: String,
        #[builder(default)] kind: BuildLogKind,
        #[builder(into)] log: String,
        successful: bool,
    ) -> Result<()> {
        self.publish_build_logs(
            conn,
            storage,
            [NewBuildLog {
                target,
                kind,
                log,
                successful,
            }],
        )
        .await
    }

    /// Upload up to eight logs concurrently, then register all successful uploads
    /// in one SQL statement using the supplied connection. Upload errors are reported
    /// after registration so other logs remain available even when one fails.
    pub async fn publish_build_logs(
        &mut self,
        conn: &mut sqlx::PgConnection,
        storage: &docs_rs_storage::AsyncStorage,
        logs: impl IntoIterator<Item = NewBuildLog>,
    ) -> Result<()> {
        let id = self.id;
        let results = stream::iter(logs)
            .map(|log| async move {
                let filename = log.filename();
                storage.store_one(log.storage_path(id), log.log).await?;
                Ok::<_, anyhow::Error>((filename, log.successful))
            })
            .buffer_unordered(8)
            .collect::<Vec<_>>()
            .await;

        let successful_uploads: Vec<(String, bool)> = results
            .iter()
            .filter_map(|result| {
                let Ok(result) = &result else {
                    return None;
                };
                Some(result.clone())
            })
            .collect();

        if !successful_uploads.is_empty() {
            self.register_logs(&mut *conn, successful_uploads).await?;
        }

        if let Some(err_result) = results.into_iter().find(|result| result.is_err()) {
            return Err(err_result.unwrap_err());
        }

        Ok(())
    }

    async fn register_logs(
        &mut self,
        conn: &mut sqlx::PgConnection,
        build_logs: impl IntoIterator<Item = (String, bool)>,
    ) -> Result<()> {
        let (logs_filename, successes): (Vec<String>, Vec<bool>) = build_logs.into_iter().unzip();

        sqlx::query!(
            "INSERT INTO builds_logs(build_id, log_filename, success)
         SELECT $1, * FROM UNNEST($2::text[], $3::bool[])
         ON CONFLICT (build_id, log_filename) DO UPDATE SET success = EXCLUDED.success",
            self.id as _,
            &logs_filename as &[String],
            &successes as &[bool],
        )
        .execute(conn)
        .await?;
        // Mirror the upsert only after SQL succeeds, preserving the same ordering
        // as reads. Failed uploads and failed registrations leave these untouched.
        for (filename, successful) in logs_filename.into_iter().zip(successes) {
            match self
                .logs
                .binary_search_by(|(existing, _)| existing.cmp(&filename))
            {
                Ok(index) => self.logs[index].1 = successful,
                Err(index) => self.logs.insert(index, (filename, successful)),
            }
        }
        Ok(())
    }

    /// Configure completion data, then persist the transition with `.save(conn).await`.
    #[builder(finish_fn = save)]
    pub async fn finish(
        self,
        #[builder(finish_fn)] conn: &mut sqlx::PgConnection,
        rustc_version: &str,
        docsrs_version: &str,
        successful: bool,
        documentation_size: Option<ByteSize>,
        memory_peak: Option<ByteSize>,
        #[builder(with = |error: &impl BuildError| CompletionError {
            message: error.to_string(),
            kind: error.kind(),
        })]
        error: Option<CompletionError>,
    ) -> Result<Build<Finished>> {
        let mut transaction = sqlx::Connection::begin(conn).await?;
        self.lock_in_progress(&mut transaction).await?;
        let status = if successful {
            BuildStatus::Success
        } else {
            BuildStatus::Failure
        };
        let conn = &mut *transaction;
        debug!("updating build after finishing");
        let hostname = hostname::get()?;

        let rustc_date = match parse_rustc_date(rustc_version) {
            Ok(date) => Some(date),
            Err(err) => {
                // in the database we see cases where the rustc version is missing
                // in the builds-table. In this case & if we can't parse the version
                // we just want to log an error, but still finish the build.
                error!(
                    "Failed to parse date from rustc version \"{}\": {:?}",
                    rustc_version, err
                );
                None
            }
        };

        let release_id = sqlx::query_scalar!(
            r#"UPDATE builds
         SET
             rustc_version = $1,
             docsrs_version = $2,
             build_status = $3,
             build_server = $4,
             errors = $5,
             documentation_size = $6,
             rustc_nightly_date = $7,
             build_finished = NOW(),
             error_kind = $8,
             memory_peak = $9
         WHERE
            id = $10
         RETURNING rid as "rid: ReleaseId" "#,
            rustc_version,
            docsrs_version,
            status as BuildStatus,
            hostname.to_str().unwrap_or(""),
            error.as_ref().map(|err| err.to_string()),
            documentation_size as _,
            rustc_date,
            error.as_ref().map(|err| err.kind()),
            memory_peak as _,
            self.id as _,
        )
        .fetch_one(&mut *conn)
        .await?;

        update_build_status(conn, release_id).await?;

        let AnyBuild::Finished(build) = AnyBuild::open(conn, self.id).await? else {
            unreachable!("just completed build is finished");
        };
        transaction.commit().await?;
        Ok(build)
    }

    /// Configure an early failure, then persist it with `.save(conn).await`.
    #[builder(finish_fn = save)]
    pub async fn fail_early(
        self,
        #[builder(finish_fn)] conn: &mut sqlx::PgConnection,
        #[builder(with = |error: &impl BuildError| CompletionError {
            message: error.to_string(),
            kind: error.kind(),
        })]
        error: Option<CompletionError>,
    ) -> Result<Build<EarlyFailure>> {
        let mut transaction = sqlx::Connection::begin(conn).await?;
        self.lock_in_progress(&mut transaction).await?;
        let conn = &mut *transaction;
        debug!("updating build with error");
        let release_id = sqlx::query_scalar!(
            r#"UPDATE builds
         SET
             build_status = $1,
             errors = $2,
             error_kind = $3
         WHERE id = $4
         RETURNING rid as "rid: ReleaseId" "#,
            BuildStatus::Failure as BuildStatus,
            error.as_ref().map(|err| err.to_string()),
            error.as_ref().map(|err| err.kind()),
            self.id.0,
        )
        .fetch_one(&mut *conn)
        .await?;

        update_build_status(conn, release_id).await?;

        let AnyBuild::EarlyFailure(build) = AnyBuild::open(conn, self.id).await? else {
            unreachable!("just failed build is an early failure");
        };
        transaction.commit().await?;
        Ok(build)
    }

    /// Recheck under a row lock: another process may have completed this
    /// attempt since the handle was created. Hold the lock through the write
    /// and release status update, so the transition is atomic.
    async fn lock_in_progress(&self, conn: &mut sqlx::PgConnection) -> Result<()> {
        let status = sqlx::query_scalar::<_, BuildStatus>(
            "SELECT build_status FROM builds WHERE id = $1 FOR UPDATE",
        )
        .bind(self.id.0)
        .fetch_one(conn)
        .await?;
        anyhow::ensure!(
            status == BuildStatus::InProgress,
            "build {} is no longer in progress",
            self.id
        );
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        Config,
        releases::{initialize_crate, initialize_release},
        testing::TestDatabase,
    };
    use docs_rs_config::AppConfig as _;
    use docs_rs_opentelemetry::testing::TestMetrics;
    use docs_rs_types::testing::{KRATE, V0_1};

    #[tokio::test(flavor = "multi_thread")]
    async fn log_reads_support_storage_fallback_and_legacy_output() -> Result<()> {
        let metrics = TestMetrics::new();
        let storage = docs_rs_storage::testing::TestStorage::from_kind(
            docs_rs_storage::StorageKind::Memory,
            metrics.provider(),
        )
        .await?;
        let db = TestDatabase::new(&Config::test_config()?, metrics.provider()).await?;
        let mut conn = db.async_conn().await?;
        let crate_id = initialize_crate(&mut conn, &KRATE).await?;
        let release_id = initialize_release(&mut conn, crate_id, &V0_1).await?;
        let build = AnyBuild::start(&mut conn, release_id).await?;
        for filename in ["b.txt", "a.txt"] {
            storage
                .store_one(
                    format!("build-logs/{}/{filename}", build.id()),
                    filename.to_owned(),
                )
                .await?;
        }
        let listed = build.list_build_logs(&storage).await?;
        assert_eq!(listed[0].fetch(&storage).await?, "a.txt");
        assert_eq!(
            listed
                .into_iter()
                .map(|log| (log.filename().to_owned(), log.successful()))
                .collect::<Vec<_>>(),
            vec![("a.txt".into(), None), ("b.txt".into(), None)]
        );
        assert_eq!(build.build_log("a.txt").fetch(&storage).await?, "a.txt");
        let error = build
            .build_log("missing.txt")
            .fetch(&storage)
            .await
            .unwrap_err();
        assert!(error.is::<docs_rs_storage::PathNotFoundError>());

        sqlx::query("UPDATE builds SET output = 'legacy log' WHERE id = $1")
            .bind(build.id().0)
            .execute(&mut *conn)
            .await?;
        let legacy = AnyBuild::open(&mut conn, build.id()).await?;
        assert!(legacy.list_build_logs(&storage).await?.is_empty());
        assert_eq!(legacy.fetch_legacy_output(db.pool()).await?, "legacy log");
        // Content is read at fetch time, not carried by the build snapshot.
        sqlx::query("UPDATE builds SET output = 'updated log' WHERE id = $1")
            .bind(build.id().0)
            .execute(&mut *conn)
            .await?;
        assert_eq!(legacy.fetch_legacy_output(db.pool()).await?, "updated log");
        sqlx::query("UPDATE builds SET output = NULL WHERE id = $1")
            .bind(build.id().0)
            .execute(&mut *conn)
            .await?;
        assert!(
            legacy
                .fetch_legacy_output(db.pool())
                .await
                .unwrap_err()
                .is::<docs_rs_storage::PathNotFoundError>()
        );
        Ok(())
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn typed_reads_preserve_metadata_scope_and_legacy_builds() -> Result<()> {
        let metrics = TestMetrics::new();
        let storage = docs_rs_storage::testing::TestStorage::from_kind(
            docs_rs_storage::StorageKind::Memory,
            metrics.provider(),
        )
        .await?;
        let db = TestDatabase::new(&Config::test_config()?, metrics.provider()).await?;
        let mut conn = db.async_conn().await?;
        let crate_id = initialize_crate(&mut conn, &KRATE).await?;
        let release_id = initialize_release(&mut conn, crate_id, &V0_1).await?;
        let finished = AnyBuild::start(&mut conn, release_id)
            .await?
            .finish()
            .rustc_version("rustc 1.84.0-nightly (e7c0d2750 2024-10-15)")
            .docsrs_version("docs.rs test")
            .successful(true)
            .memory_peak(ByteSize::mib(12))
            .save(&mut conn)
            .await?;
        assert_eq!(
            finished.state().docsrs_version.as_deref(),
            Some("docs.rs test")
        );
        assert!(finished.state().finished_at.is_some());
        assert!(finished.started_at().is_some());
        let id = finished.id();
        sqlx::query("INSERT INTO builds_logs (build_id, log_filename, success) VALUES ($1, 'target.txt', false)")
            .bind(id.0).execute(&mut *conn).await?;
        sqlx::query("UPDATE builds SET output = 'legacy log' WHERE id = $1")
            .bind(id.0)
            .execute(&mut *conn)
            .await?;

        let detail = AnyBuild::find_for_release(&mut conn, &KRATE, &V0_1, id)
            .await?
            .unwrap();
        assert_eq!(detail.status(), BuildStatus::Success);
        assert_eq!(detail.display_status(), BuildStatus::PartialFailure);
        let AnyBuild::Finished(detail) = detail else {
            panic!("completed build should be finished");
        };
        assert!(detail.has_legacy_output());
        assert_eq!(detail.logs(), &[("target.txt".into(), false)]);
        assert!(
            AnyBuild::find_for_release(&mut conn, &KRATE, &docs_rs_types::testing::V1, id)
                .await?
                .is_none()
        );

        let in_progress = AnyBuild::start(&mut conn, release_id).await?;
        let builds = AnyBuild::for_release(&mut conn, &KRATE, &V0_1).await?;
        assert_eq!(
            builds.iter().map(AnyBuild::id).collect::<Vec<_>>(),
            vec![in_progress.id(), id]
        );
        assert!(!builds[0].has_legacy_output());
        assert!(builds[1].has_legacy_output());
        assert!(builds[1].list_build_logs(&storage).await?.is_empty());
        assert_eq!(
            builds[1].fetch_legacy_output(db.pool()).await?,
            "legacy log"
        );
        assert!(matches!(&builds[0], AnyBuild::InProgress(_)));
        assert!(builds[0].duration(Utc::now()).is_some());

        // Successful historical rows need not have either lifecycle timestamp.
        sqlx::query("UPDATE builds SET build_started = NULL, build_finished = NULL, rustc_version = NULL WHERE id = $1")
            .bind(id.0).execute(&mut *conn).await?;
        let legacy = AnyBuild::open(&mut conn, id).await?;
        assert!(matches!(&legacy, AnyBuild::Finished(_)));
        assert!(legacy.build_time().is_none());
        assert!(legacy.duration(Utc::now()).is_none());
        Ok(())
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn batch_logs_register_successful_uploads_with_one_connection() -> Result<()> {
        let metrics = TestMetrics::new();
        let storage = docs_rs_storage::testing::TestStorage::from_kind(
            docs_rs_storage::StorageKind::Memory,
            metrics.provider(),
        )
        .await?;
        storage.reject_uploads_for_testing(Some(|path| path.ends_with("rejected")));
        let mut config = Config::test_config()?;
        config.max_pool_size = 1;
        let db = TestDatabase::new(&config, metrics.provider()).await?;
        let mut conn = db.async_conn().await?;
        let crate_id = initialize_crate(&mut conn, &KRATE).await?;
        let release_id = initialize_release(&mut conn, crate_id, &V0_1).await?;
        let mut build = AnyBuild::start(&mut conn, release_id).await?;

        // Failed uploads are omitted even when using a single connection.
        let rejected = || {
            NewBuildLog::builder()
                .target("rejected")
                .log("log")
                .successful(false)
                .build()
        };
        assert!(
            tokio::time::timeout(
                std::time::Duration::from_secs(2),
                build.publish_build_logs(&mut conn, &storage, [rejected()]),
            )
            .await?
            .is_err()
        );

        let logs = [
            rejected(),
            NewBuildLog::builder()
                .target("target")
                .log("html")
                .successful(true)
                .build(),
            NewBuildLog::builder()
                .target("target")
                .kind(BuildLogKind::Json)
                .log("json")
                .successful(false)
                .build(),
        ];
        assert!(
            tokio::time::timeout(
                std::time::Duration::from_secs(2),
                build.publish_build_logs(&mut conn, &storage, logs),
            )
            .await?
            .is_err()
        );
        let logs = sqlx::query_as::<_, (String, bool)>(
            "SELECT log_filename, success FROM builds_logs WHERE build_id = $1 ORDER BY log_filename",
        )
        .bind(build.id().0)
        .fetch_all(&mut *conn)
        .await?;
        assert_eq!(
            logs,
            vec![("target".into(), true), ("target_json".into(), false)]
        );
        assert_eq!(build.logs(), logs.as_slice());
        assert_eq!(
            build
                .list_build_logs(&storage)
                .await?
                .into_iter()
                .map(|log| (log.filename().to_owned(), log.successful()))
                .collect::<Vec<_>>(),
            vec![
                ("target".into(), Some(true)),
                ("target_json".into(), Some(false))
            ]
        );
        assert_eq!(build.build_log("target").fetch(&storage).await?, "html");
        for (filename, _) in logs {
            assert!(
                storage
                    .exists(&format!("build-logs/{}/{filename}", build.id()))
                    .await?
            );
        }
        Ok(())
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn stale_handle_cannot_overwrite_completed_build() -> Result<()> {
        let metrics = TestMetrics::new();
        let db = TestDatabase::new(&Config::test_config()?, metrics.provider()).await?;
        let mut conn = db.async_conn().await?;
        let crate_id = initialize_crate(&mut conn, &KRATE).await?;
        let release_id = initialize_release(&mut conn, crate_id, &V0_1).await?;
        let build = AnyBuild::start(&mut conn, release_id).await?;
        let id = build.id();
        let AnyBuild::InProgress(stale) = AnyBuild::open(&mut conn, id).await? else {
            panic!("new attempt should be in progress");
        };
        build
            .finish()
            .rustc_version("rustc 1.84.0-nightly (e7c0d2750 2024-10-15)")
            .docsrs_version("docs.rs test")
            .successful(true)
            .save(&mut conn)
            .await?;
        assert!(stale.fail_early().save(&mut conn).await.is_err());
        let AnyBuild::Finished(finished) = AnyBuild::open(&mut conn, id).await? else {
            panic!("completed attempt should remain finished");
        };
        assert_eq!(finished.state().status, BuildStatus::Success);
        Ok(())
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn open_distinguishes_early_and_finished_failures() -> Result<()> {
        let metrics = TestMetrics::new();
        let db = TestDatabase::new(&Config::test_config()?, metrics.provider()).await?;
        let mut conn = db.async_conn().await?;
        let crate_id = initialize_crate(&mut conn, &KRATE).await?;
        let release_id = initialize_release(&mut conn, crate_id, &V0_1).await?;
        let early = AnyBuild::start(&mut conn, release_id)
            .await?
            .fail_early()
            .save(&mut conn)
            .await?;
        assert!(matches!(
            AnyBuild::open(&mut conn, early.id()).await?,
            AnyBuild::EarlyFailure(_)
        ));
        let finished = AnyBuild::start(&mut conn, release_id)
            .await?
            .finish()
            .rustc_version("rustc 1.84.0-nightly (e7c0d2750 2024-10-15)")
            .docsrs_version("docs.rs test")
            .successful(false)
            .save(&mut conn)
            .await?;
        assert!(matches!(
            AnyBuild::open(&mut conn, finished.id()).await?,
            AnyBuild::Finished(_)
        ));
        Ok(())
    }
}
