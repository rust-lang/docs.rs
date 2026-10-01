//! Typed build handles and lifecycle transitions.
//!
//! A transition consumes its in-progress handle and checks the database state.

use crate::releases::update_build_status;
use anyhow::{Result, anyhow};
use chrono::{DateTime, Utc};
use docs_rs_storage::{AsyncStorage, PathNotFoundError, StreamingBlob};
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
    fn ext(&self) -> &'static str {
        return "txt";
    }
}

fn build_log_filename(target: &str, kind: BuildLogKind) -> String {
    format!("{target}{}.{}", kind.suffix(), kind.ext())
}

fn build_log_storage_path(build_id: BuildId, filename: &str) -> String {
    format!("build-logs/{build_id}/{filename}")
}

async fn collect_log_uploads<F: std::future::Future>(
    uploads: impl IntoIterator<Item = F>,
) -> Vec<F::Output> {
    stream::iter(uploads).buffer_unordered(8).collect().await
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
        build_log_filename(&self.target, self.kind)
    }

    fn storage_path(&self, build_id: BuildId) -> String {
        build_log_storage_path(build_id, &build_log_filename(&self.target, self.kind))
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

    pub async fn fetch(&self, storage: &AsyncStorage) -> Result<StreamingBlob> {
        storage.get_stream(&self.storage_path).await
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
    SELECT
        b.id,
        b.build_status,
        b.build_started,
        b.build_finished,
        b.rustc_version,
        b.docsrs_version,
        b.memory_peak,
        b.documentation_size,
        b.errors,
        b.error_kind,
        b.output IS NOT NULL AS has_legacy_output,
        r.default_target,
        (
            SELECT array_agg(row(l.log_filename, l.success) ORDER BY l.log_filename)
            FROM builds_logs l WHERE l.build_id = b.id
        ) AS logs
    FROM builds b
    INNER JOIN releases r ON r.id = b.rid
    INNER JOIN crates c ON c.id = r.crate_id
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

// Keep forwarding signatures visible while sharing the variant dispatch.
macro_rules! forward_build_methods {
    ($($(#[$attr:meta])* $vis:vis fn $name:ident(&self $(, $arg:ident: $arg_ty:ty)*) -> $ret:ty;)*) => {
        $(
            $(#[$attr])*
            $vis fn $name(&self $(, $arg: $arg_ty)*) -> $ret {
                match self {
                    Self::InProgress(build) => build.$name($($arg),*),
                    Self::Finished(build) => build.$name($($arg),*),
                    Self::EarlyFailure(build) => build.$name($($arg),*),
                }
            }
        )*
    };
}

impl AnyBuild {
    forward_build_methods! {
        pub fn id(&self) -> BuildId;
        pub fn started_at(&self) -> Option<DateTime<Utc>>;
        pub fn logs(&self) -> &[(String, bool)];
        pub fn has_legacy_output(&self) -> bool;
        pub fn default_target(&self) -> Option<&str>;
        pub fn build_log(&self, filename: &str) -> BuildLog;
    }

    pub async fn fetch_legacy_output(&self, conn: &mut sqlx::PgConnection) -> Result<String> {
        match self {
            Self::InProgress(build) => build.fetch_legacy_output(conn).await,
            Self::Finished(build) => build.fetch_legacy_output(conn).await,
            Self::EarlyFailure(build) => build.fetch_legacy_output(conn).await,
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
            .map(|default_target| build_log_filename(default_target, BuildLogKind::Html))
    }

    pub async fn list_build_logs(&self, storage: &AsyncStorage) -> Result<Vec<BuildLog>> {
        match self {
            Self::InProgress(build) => build.list_build_logs(storage).await,
            Self::Finished(build) => build.list_build_logs(storage).await,
            Self::EarlyFailure(build) => build.list_build_logs(storage).await,
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

    pub fn duration(&self) -> Option<Duration> {
        self.duration_at(Utc::now())
    }

    /// Compute elapsed time at a supplied instant for in-progress builds.
    pub fn duration_at(&self, now: DateTime<Utc>) -> Option<Duration> {
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
    /// Describe a storage log, including its registered status when available.
    pub fn build_log(&self, filename: &str) -> BuildLog {
        BuildLog {
            filename: filename.into(),
            successful: self
                .logs
                .iter()
                .find(|(name, _)| name == filename)
                .map(|(_, successful)| *successful),
            storage_path: build_log_storage_path(self.id, filename),
        }
    }

    /// Fetch legacy database output lazily. Missing output is a not-found error.
    pub async fn fetch_legacy_output(&self, conn: &mut sqlx::PgConnection) -> Result<String> {
        sqlx::query_scalar::<_, Option<String>>("SELECT output FROM builds WHERE id = $1")
            .bind(self.id.0)
            .fetch_optional(&mut *conn)
            .await?
            .flatten()
            .ok_or_else(|| PathNotFoundError.into())
    }

    /// List registered logs, falling back to storage for older attempts without
    /// log records. Legacy database output is fetched separately and has no files.
    pub async fn list_build_logs(&self, storage: &AsyncStorage) -> Result<Vec<BuildLog>> {
        if self.has_legacy_output {
            return Ok(Vec::new());
        }
        if !self.logs.is_empty() {
            Ok(self
                .logs
                .iter()
                .map(|(filename, success)| BuildLog {
                    filename: filename.into(),
                    successful: Some(*success),
                    storage_path: build_log_storage_path(self.id, filename),
                })
                .collect())
        } else {
            let prefix = format!("build-logs/{}/", self.id);
            storage
                .list_prefix(&prefix)
                .await
                .map_ok(|path| BuildLog {
                    filename: path
                        .strip_prefix(&prefix)
                        .expect("storage lists only keys under the requested prefix")
                        .into(),
                    storage_path: path,
                    successful: None,
                })
                .try_collect()
                .await
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
        #[builder(finish_fn)] storage: &AsyncStorage,
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
        storage: &AsyncStorage,
        logs: impl IntoIterator<Item = NewBuildLog>,
    ) -> Result<()> {
        let id = self.id;
        let results = collect_log_uploads(logs.into_iter().map(|log| async move {
            let filename = log.filename();
            storage.store_one(log.storage_path(id), log.log).await?;
            Ok::<_, anyhow::Error>((filename, log.successful))
        }))
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
    use chrono::NaiveDate;
    use docs_rs_config::AppConfig as _;
    use docs_rs_opentelemetry::testing::TestMetrics;
    use docs_rs_storage::{StorageKind, testing::TestStorage};
    use docs_rs_types::{
        SimpleBuildError,
        testing::{KRATE, V0_1, V1},
    };

    fn snapshot<State>(state: State) -> Build<State> {
        Build {
            id: BuildId(1),
            started_at: DateTime::from_timestamp(1_000, 0),
            logs: Vec::new(),
            has_legacy_output: false,
            default_target: None,
            state,
        }
    }

    #[tokio::test]
    async fn log_uploads_run_concurrently_with_a_limit_of_eight() {
        use std::sync::{
            Arc,
            atomic::{AtomicUsize, Ordering},
        };
        let active = Arc::new(AtomicUsize::new(0));
        let peak = Arc::new(AtomicUsize::new(0));
        let gate = Arc::new(tokio::sync::Semaphore::new(0));
        let (started, mut starts) = tokio::sync::mpsc::unbounded_channel();
        let uploads = (0..17)
            .map(|index| {
                let (active, peak, gate, started) =
                    (active.clone(), peak.clone(), gate.clone(), started.clone());
                async move {
                    let count = active.fetch_add(1, Ordering::SeqCst) + 1;
                    peak.fetch_max(count, Ordering::SeqCst);
                    started.send(index).unwrap();
                    gate.acquire().await.unwrap().forget();
                    active.fetch_sub(1, Ordering::SeqCst);
                    index
                }
            })
            .collect::<Vec<_>>();
        let task = tokio::spawn(collect_log_uploads(uploads));
        tokio::time::timeout(std::time::Duration::from_secs(2), async {
            for _ in 0..8 {
                starts.recv().await.unwrap();
            }
            assert!(starts.try_recv().is_err());
            assert_eq!(active.load(Ordering::SeqCst), 8);
            gate.add_permits(17);
            let mut results = task.await.unwrap();
            results.sort_unstable();
            assert_eq!(results, (0..17).collect::<Vec<_>>());
            assert_eq!(peak.load(Ordering::SeqCst), 8);
            assert_eq!(active.load(Ordering::SeqCst), 0);
        })
        .await
        .expect("uploads must make progress concurrently");
    }

    fn finished_state(status: BuildStatus) -> Finished {
        Finished {
            status,
            errors: None,
            error_kind: None,
            finished_at: DateTime::from_timestamp(1_042, 0),
            rustc_version: None,
            docsrs_version: None,
            memory_peak: None,
            documentation_size: None,
        }
    }

    #[test]
    fn finished_duration_uses_completion_not_now() {
        let mut build = snapshot(finished_state(BuildStatus::Success));
        let now = DateTime::from_timestamp(0, 0).unwrap();
        assert_eq!(
            AnyBuild::Finished(build.clone()).duration_at(now),
            Some(std::time::Duration::from_secs(42).into())
        );
        build.state.finished_at = None;
        assert_eq!(AnyBuild::Finished(build.clone()).duration_at(now), None);
        build.state.finished_at = DateTime::from_timestamp(999, 0);
        assert_eq!(AnyBuild::Finished(build.clone()).duration_at(now), None);
        build.started_at = None;
        assert_eq!(AnyBuild::Finished(build).duration_at(now), None);
        let early = AnyBuild::EarlyFailure(snapshot(EarlyFailure {
            errors: None,
            error_kind: None,
        }));
        assert_eq!(early.duration_at(now), None);
    }

    #[test]
    fn default_log_filename_requires_a_target() {
        let mut build = snapshot(InProgress);
        assert_eq!(AnyBuild::InProgress(build).default_log_filename(), None);
        build = snapshot(InProgress);
        build.default_target = Some("x86_64-unknown-linux-gnu".into());
        assert_eq!(
            AnyBuild::InProgress(build)
                .default_log_filename()
                .as_deref(),
            Some("x86_64-unknown-linux-gnu.txt")
        );
    }

    #[test]
    fn published_log_filenames_match_default_and_json_formats() {
        let target = "x86_64-unknown-linux-gnu";
        let mut build = snapshot(InProgress);
        build.default_target = Some(target.into());
        let build = AnyBuild::InProgress(build);
        for (kind, expected) in [
            (BuildLogKind::Html, "x86_64-unknown-linux-gnu.txt"),
            (BuildLogKind::Json, "x86_64-unknown-linux-gnu_json.txt"),
        ] {
            let log = NewBuildLog::builder()
                .target(target)
                .kind(kind)
                .log("output")
                .successful(true)
                .build();
            assert_eq!(log.filename(), expected);
            assert_eq!(
                log.storage_path(build.id()),
                format!("build-logs/1/{expected}")
            );
            if matches!(kind, BuildLogKind::Html) {
                assert_eq!(build.default_log_filename().as_deref(), Some(expected));
            }
        }
    }

    #[test]
    fn display_status_only_downgrades_success() {
        for logs in [
            vec![],
            vec![("a".into(), true), ("b".into(), true)],
            vec![("a".into(), true), ("b".into(), false)],
        ] {
            for status in [
                BuildStatus::Success,
                BuildStatus::Failure,
                BuildStatus::PartialFailure,
            ] {
                let mut build = snapshot(finished_state(status));
                build.logs = logs.clone();
                let expected = if status == BuildStatus::Success
                    && logs.iter().any(|(_, successful)| !successful)
                {
                    BuildStatus::PartialFailure
                } else {
                    status
                };
                assert_eq!(AnyBuild::Finished(build).display_status(), expected);
            }
            let mut build = snapshot(InProgress);
            build.logs = logs.clone();
            assert_eq!(
                AnyBuild::InProgress(build).display_status(),
                BuildStatus::InProgress
            );
            let mut build = snapshot(EarlyFailure {
                errors: None,
                error_kind: None,
            });
            build.logs = logs;
            assert_eq!(
                AnyBuild::EarlyFailure(build).display_status(),
                BuildStatus::Failure
            );
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn batch_registration_failure_preserves_snapshot_and_empty_batch_is_noop() -> Result<()> {
        let metrics = TestMetrics::new();
        let storage = TestStorage::from_kind(StorageKind::Memory, metrics.provider()).await?;
        let db = TestDatabase::new(&Config::test_config()?, metrics.provider()).await?;
        let mut conn = db.async_conn().await?;
        let crate_id = initialize_crate(&mut conn, &KRATE).await?;
        let release_id = initialize_release(&mut conn, crate_id, &V0_1).await?;
        let mut build = AnyBuild::start(&mut conn, release_id).await?;
        build
            .publish_build_log()
            .target("existing")
            .log("old")
            .successful(false)
            .save(&mut conn, &storage)
            .await?;
        let previous = build.logs().to_vec();
        build.publish_build_logs(&mut conn, &storage, []).await?;
        assert_eq!(build.logs(), previous);
        // A transaction-local trigger rejects SQL without preventing uploads.
        let mut tx = sqlx::Connection::begin(&mut *conn).await?;
        sqlx::query("CREATE FUNCTION reject_test_log() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN RAISE EXCEPTION 'injected registration failure'; END $$").execute(&mut *tx).await?;
        sqlx::query("CREATE TRIGGER reject_test_log BEFORE INSERT ON builds_logs FOR EACH ROW EXECUTE FUNCTION reject_test_log()").execute(&mut *tx).await?;
        let result = build
            .publish_build_logs(
                &mut tx,
                &storage,
                [
                    NewBuildLog::builder()
                        .target("existing")
                        .log("new")
                        .successful(true)
                        .build(),
                    NewBuildLog::builder()
                        .target("new")
                        .log("new")
                        .successful(true)
                        .build(),
                ],
            )
            .await;
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("injected registration failure")
        );
        assert_eq!(build.logs(), previous);
        tx.rollback().await?;
        let persisted = AnyBuild::open(&mut conn, build.id()).await?;
        assert_eq!(persisted.logs(), previous);
        assert!(
            storage
                .exists(&format!("build-logs/{}/new.txt", build.id()))
                .await?
        );
        Ok(())
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn completed_snapshot_preserves_metadata_and_stale_finish_is_rejected() -> Result<()> {
        let metrics = TestMetrics::new();
        let storage = TestStorage::from_kind(StorageKind::Memory, metrics.provider()).await?;
        let db = TestDatabase::new(&Config::test_config()?, metrics.provider()).await?;
        let mut conn = db.async_conn().await?;
        let crate_id = initialize_crate(&mut conn, &KRATE).await?;
        let release_id = initialize_release(&mut conn, crate_id, &V0_1).await?;
        let mut build = AnyBuild::start(&mut conn, release_id).await?;
        assert_eq!(build.default_target(), None);
        build
            .publish_build_log()
            .target("target")
            .log("log")
            .successful(false)
            .save(&mut conn, &storage)
            .await?;
        let AnyBuild::InProgress(stale) = AnyBuild::open(&mut conn, build.id()).await? else {
            panic!("expected in-progress build")
        };
        sqlx::query("UPDATE releases SET default_target = 'target' WHERE id = $1")
            .bind(release_id.0)
            .execute(&mut *conn)
            .await?;
        let error = SimpleBuildError("failure".into());
        let finished = build
            .finish()
            .rustc_version("rustc 1.84.0-nightly (e7c0d2750 2024-10-15)")
            .docsrs_version("docs.rs test")
            .successful(false)
            .memory_peak(ByteSize::mib(12))
            .documentation_size(ByteSize::b(42))
            .error(&error)
            .save(&mut conn)
            .await?;
        assert_eq!(finished.default_target(), Some("target"));
        assert_eq!(finished.logs(), &[("target.txt".into(), false)]);
        assert_eq!(
            finished.state().errors.as_deref(),
            Some("build error: failure")
        );
        assert_eq!(
            finished.state().error_kind.as_deref(),
            Some("SimpleBuildError")
        );
        assert_eq!(finished.state().memory_peak, Some(ByteSize::mib(12)));
        assert_eq!(finished.state().documentation_size, Some(ByteSize::b(42)));
        assert!(
            stale
                .finish()
                .rustc_version("stale")
                .docsrs_version("stale")
                .successful(true)
                .save(&mut conn)
                .await
                .is_err()
        );
        let AnyBuild::Finished(reopened) = AnyBuild::open(&mut conn, finished.id()).await? else {
            panic!("expected finished build")
        };
        assert_eq!(reopened, finished);
        let early = AnyBuild::start(&mut conn, release_id)
            .await?
            .fail_early()
            .error(&error)
            .save(&mut conn)
            .await?;
        assert_eq!(early.default_target(), Some("target"));
        assert_eq!(
            early.state().errors.as_deref(),
            Some("build error: failure")
        );
        assert_eq!(
            early.state().error_kind.as_deref(),
            Some("SimpleBuildError")
        );
        Ok(())
    }

    #[test]
    fn in_progress_duration_at() {
        let started_at = DateTime::from_timestamp(1_000, 0).unwrap();
        let mut build = AnyBuild::InProgress(Build {
            id: BuildId(1),
            started_at: Some(started_at),
            logs: Vec::new(),
            has_legacy_output: false,
            default_target: None,
            state: InProgress,
        });
        assert_eq!(
            build.duration_at(started_at + chrono::Duration::seconds(42)),
            Some(std::time::Duration::from_secs(42).into())
        );
        assert_eq!(
            build.duration_at(started_at),
            Some(std::time::Duration::ZERO.into())
        );
        assert_eq!(
            build.duration_at(started_at - chrono::Duration::seconds(1)),
            None
        );
        if let AnyBuild::InProgress(build) = &mut build {
            build.started_at = None;
        }
        assert_eq!(build.duration_at(started_at), None);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn log_reads_support_storage_fallback_and_legacy_output() -> Result<()> {
        let metrics = TestMetrics::new();
        let storage = TestStorage::from_kind(StorageKind::Memory, metrics.provider()).await?;
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
        assert_eq!(
            listed[0]
                .fetch(&storage)
                .await?
                .materialize(ByteSize::MAX)
                .await?
                .content,
            b"a.txt"
        );
        assert_eq!(
            listed
                .into_iter()
                .map(|log| (log.filename().to_owned(), log.successful()))
                .collect::<Vec<_>>(),
            vec![("a.txt".into(), None), ("b.txt".into(), None)]
        );
        assert_eq!(
            build
                .build_log("a.txt")
                .fetch(&storage)
                .await?
                .materialize(ByteSize::MAX)
                .await?
                .content,
            b"a.txt"
        );
        let error = build
            .build_log("missing.txt")
            .fetch(&storage)
            .await
            .unwrap_err();
        assert!(error.is::<PathNotFoundError>());

        sqlx::query("UPDATE builds SET output = 'legacy log' WHERE id = $1")
            .bind(build.id().0)
            .execute(&mut *conn)
            .await?;
        let legacy = AnyBuild::open(&mut conn, build.id()).await?;
        assert!(legacy.list_build_logs(&storage).await?.is_empty());
        assert_eq!(legacy.fetch_legacy_output(&mut conn).await?, "legacy log");
        // Content is read at fetch time, not carried by the build snapshot.
        sqlx::query("UPDATE builds SET output = 'updated log' WHERE id = $1")
            .bind(build.id().0)
            .execute(&mut *conn)
            .await?;
        assert_eq!(legacy.fetch_legacy_output(&mut conn).await?, "updated log");
        sqlx::query("UPDATE builds SET output = NULL WHERE id = $1")
            .bind(build.id().0)
            .execute(&mut *conn)
            .await?;
        assert!(
            legacy
                .fetch_legacy_output(&mut conn)
                .await
                .unwrap_err()
                .is::<PathNotFoundError>()
        );
        Ok(())
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn typed_reads_preserve_metadata_scope_and_legacy_builds() -> Result<()> {
        let metrics = TestMetrics::new();
        let storage = TestStorage::from_kind(StorageKind::Memory, metrics.provider()).await?;
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
            builds[1].fetch_legacy_output(&mut conn).await?,
            "legacy log"
        );
        assert!(matches!(&builds[0], AnyBuild::InProgress(_)));
        assert!(builds[0].duration().is_some());

        // Successful historical rows need not have either lifecycle timestamp.
        sqlx::query("UPDATE builds SET build_started = NULL, build_finished = NULL, rustc_version = NULL WHERE id = $1")
            .bind(id.0).execute(&mut *conn).await?;
        let legacy = AnyBuild::open(&mut conn, id).await?;
        assert!(matches!(&legacy, AnyBuild::Finished(_)));
        assert!(legacy.build_time().is_none());
        assert!(legacy.duration().is_none());
        Ok(())
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn batch_logs_register_successful_uploads_with_one_connection() -> Result<()> {
        let metrics = TestMetrics::new();
        let storage = TestStorage::from_kind(StorageKind::Memory, metrics.provider()).await?;
        storage.reject_uploads_for_testing(Some(|path| path.ends_with("rejected.txt")));
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
            vec![
                ("target.txt".into(), true),
                ("target_json.txt".into(), false)
            ]
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
                ("target.txt".into(), Some(true)),
                ("target_json.txt".into(), Some(false))
            ]
        );
        assert_eq!(
            build
                .build_log("target.txt")
                .fetch(&storage)
                .await?
                .materialize(ByteSize::MAX)
                .await?
                .content,
            b"html"
        );
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
    #[tokio::test(flavor = "multi_thread")]
    async fn build_log_registration_is_idempotent() -> Result<()> {
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
        let mut build = AnyBuild::start(&mut conn, release_id).await?;
        let build_id = build.id();
        build
            .publish_build_log()
            .target("target")
            .log("failed")
            .successful(false)
            .save(&mut conn, &storage)
            .await?;
        build
            .publish_build_log()
            .target("target")
            .log("succeeded")
            .successful(true)
            .save(&mut conn, &storage)
            .await?;
        let logs = sqlx::query_as::<_, (String, bool)>(
            "SELECT log_filename, success FROM builds_logs WHERE build_id = $1",
        )
        .bind(build_id.0)
        .fetch_all(&mut *conn)
        .await?;
        assert_eq!(logs, vec![("target.txt".into(), true)]);
        assert_eq!(build.logs(), logs.as_slice());
        Ok(())
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_set_build_to_error() -> Result<()> {
        let test_metrics = TestMetrics::new();
        let db = TestDatabase::new(&Config::test_config()?, test_metrics.provider()).await?;

        let mut conn = db.async_conn().await?;
        let crate_id = initialize_crate(&mut conn, &KRATE).await?;
        let release_id = initialize_release(&mut conn, crate_id, &V0_1).await?;
        let build = AnyBuild::start(&mut conn, release_id).await?;
        let build_id = build.id();

        build
            .fail_early()
            .maybe_error(Some(&SimpleBuildError("error message".into())))
            .save(&mut conn)
            .await?;

        let row = sqlx::query!(
            r#"SELECT
                rustc_version,
                docsrs_version,
                build_started,
                build_status as "build_status: BuildStatus",
                errors,
                error_kind
               FROM builds
               WHERE id = $1"#,
            build_id as _
        )
        .fetch_one(&mut *conn)
        .await?;

        assert!(row.rustc_version.is_none());
        assert!(row.docsrs_version.is_none());
        assert!(row.build_started.is_some());
        assert_eq!(row.build_status, BuildStatus::Failure);
        assert_eq!(row.errors, Some("build error: error message".into()));
        assert_eq!(row.error_kind, Some("SimpleBuildError".into()));

        Ok(())
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_finish_build_success_valid_rustc_date() -> Result<()> {
        let test_metrics = TestMetrics::new();
        let db = TestDatabase::new(&Config::test_config()?, test_metrics.provider()).await?;

        let mut conn = db.async_conn().await?;
        let crate_id = initialize_crate(&mut conn, &KRATE).await?;
        let release_id = initialize_release(&mut conn, crate_id, &V0_1).await?;
        let build = AnyBuild::start(&mut conn, release_id).await?;
        let build_id = build.id();

        build
            .finish()
            .rustc_version("rustc 1.84.0-nightly (e7c0d2750 2024-10-15)")
            .docsrs_version("docsrs_version")
            .successful(BuildStatus::Success == BuildStatus::Success)
            .save(&mut conn)
            .await?;

        let row = sqlx::query!(
            r#"SELECT
                rustc_version,
                docsrs_version,
                build_status as "build_status: BuildStatus",
                errors,
                error_kind,
                rustc_nightly_date
                FROM builds
                WHERE id = $1"#,
            build_id.0
        )
        .fetch_one(&mut *conn)
        .await?;

        assert_eq!(
            row.rustc_version,
            Some("rustc 1.84.0-nightly (e7c0d2750 2024-10-15)".into())
        );
        assert_eq!(row.docsrs_version, Some("docsrs_version".into()));
        assert_eq!(row.build_status, BuildStatus::Success);
        assert_eq!(
            row.rustc_nightly_date,
            Some(NaiveDate::from_ymd_opt(2024, 10, 15).unwrap())
        );
        assert!(row.errors.is_none());
        assert!(row.error_kind.is_none());

        Ok(())
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_finish_build_success_invalid_rustc_date() -> Result<()> {
        let test_metrics = TestMetrics::new();
        let db = TestDatabase::new(&Config::test_config()?, test_metrics.provider()).await?;

        let mut conn = db.async_conn().await?;
        let crate_id = initialize_crate(&mut conn, &KRATE).await?;
        let release_id = initialize_release(&mut conn, crate_id, &V0_1).await?;
        let build = AnyBuild::start(&mut conn, release_id).await?;
        let build_id = build.id();

        build
            .finish()
            .rustc_version("rustc_version")
            .docsrs_version("docsrs_version")
            .successful(BuildStatus::Success == BuildStatus::Success)
            .maybe_documentation_size(Some(42u64.into()))
            .maybe_memory_peak(Some(23u64.into()))
            .save(&mut conn)
            .await?;

        let row = sqlx::query!(
            r#"SELECT
                rustc_version,
                docsrs_version,
                build_status as "build_status: BuildStatus",
                documentation_size,
                memory_peak,
                errors,
                error_kind,
                rustc_nightly_date
                FROM builds
                WHERE id = $1"#,
            build_id.0
        )
        .fetch_one(&mut *conn)
        .await?;

        assert_eq!(row.rustc_version, Some("rustc_version".into()));
        assert_eq!(row.docsrs_version, Some("docsrs_version".into()));
        assert_eq!(row.build_status, BuildStatus::Success);
        assert_eq!(row.documentation_size, Some(42));
        assert_eq!(row.memory_peak, Some(23));
        assert!(row.rustc_nightly_date.is_none());
        assert!(row.errors.is_none());
        assert!(row.error_kind.is_none());

        Ok(())
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_finish_build_error() -> Result<()> {
        let test_metrics = TestMetrics::new();
        let db = TestDatabase::new(&Config::test_config()?, test_metrics.provider()).await?;

        let mut conn = db.async_conn().await?;
        let crate_id = initialize_crate(&mut conn, &KRATE).await?;
        let release_id = initialize_release(&mut conn, crate_id, &V0_1).await?;
        let build = AnyBuild::start(&mut conn, release_id).await?;
        let build_id = build.id();

        build
            .finish()
            .rustc_version("rustc_version")
            .docsrs_version("docsrs_version")
            .successful(BuildStatus::Failure == BuildStatus::Success)
            .maybe_error(Some(&SimpleBuildError("error message".into())))
            .save(&mut conn)
            .await?;

        let row = sqlx::query!(
            r#"SELECT
                rustc_version,
                docsrs_version,
                build_status as "build_status: BuildStatus",
                documentation_size,
                errors,
                error_kind
               FROM builds
               WHERE id = $1"#,
            build_id as _
        )
        .fetch_one(&mut *conn)
        .await?;

        assert_eq!(row.rustc_version, Some("rustc_version".into()));
        assert_eq!(row.docsrs_version, Some("docsrs_version".into()));
        assert_eq!(row.build_status, BuildStatus::Failure);
        assert_eq!(row.errors, Some("build error: error message".into()));
        assert_eq!(row.error_kind, Some("SimpleBuildError".into()));
        assert!(row.documentation_size.is_none());

        Ok(())
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_initialize_build() -> Result<()> {
        let test_metrics = TestMetrics::new();
        let db = TestDatabase::new(&Config::test_config()?, test_metrics.provider()).await?;

        let mut conn = db.async_conn().await?;
        let crate_id = initialize_crate(&mut conn, &KRATE).await?;
        let release_id = initialize_release(&mut conn, crate_id, &V1).await?;

        let build = AnyBuild::start(&mut conn, release_id).await?;
        let build_id = build.id();

        let id = sqlx::query_scalar!(
            r#"SELECT id as "id: BuildId" FROM builds WHERE rid = $1"#,
            release_id.0
        )
        .fetch_one(&mut *conn)
        .await?;

        assert_eq!(build_id, id);

        let another_build_id = AnyBuild::start(&mut conn, release_id).await?.id();
        assert_ne!(build_id, another_build_id);

        Ok(())
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_initialize_build_marks_previous_attempt_as_failure() -> Result<()> {
        let test_metrics = TestMetrics::new();
        let db = TestDatabase::new(&Config::test_config()?, test_metrics.provider()).await?;

        let mut conn = db.async_conn().await?;
        let crate_id = initialize_crate(&mut conn, &KRATE).await?;
        let release_id = initialize_release(&mut conn, crate_id, &V1).await?;

        let first_build_id = AnyBuild::start(&mut conn, release_id).await?.id();
        let second_build_id = AnyBuild::start(&mut conn, release_id).await?.id();

        assert_ne!(first_build_id, second_build_id);

        let builds = sqlx::query!(
            r#"SELECT
                id as "id: BuildId",
                build_status as "build_status: BuildStatus",
                errors,
                build_finished
               FROM builds
               WHERE rid = $1
               ORDER BY id ASC"#,
            release_id.0,
        )
        .fetch_all(&mut *conn)
        .await?;

        assert_eq!(builds.len(), 2);

        assert_eq!(builds[0].id, first_build_id);
        assert_eq!(builds[0].build_status, BuildStatus::Failure);
        assert_eq!(
            builds[0].errors,
            Some("build aborted: builder process restarted before completion".into())
        );
        assert!(builds[0].build_finished.is_some());

        assert_eq!(builds[1].id, second_build_id);
        assert_eq!(builds[1].build_status, BuildStatus::InProgress);
        assert!(builds[1].build_finished.is_none());

        Ok(())
    }
}
