//! Typed build handles and lifecycle transitions.
//!
//! A transition consumes its in-progress handle and checks the database state.

use crate::releases::update_build_status;
use anyhow::{Result, anyhow};
use docs_rs_types::{BuildError, BuildId, BuildStatus, ByteSize, ReleaseId};
use docs_rs_utils::rustc_version::parse_rustc_date;
use tracing::{debug, error};

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
    async fn stale_handle_cannot_overwrite_completed_build() -> Result<()> {
        let metrics = TestMetrics::new();
        let db = TestDatabase::new(&Config::test_config()?, metrics.provider()).await?;
        let mut conn = db.async_conn().await?;
        let crate_id = initialize_crate(&mut conn, &KRATE).await?;
        let release_id = initialize_release(&mut conn, crate_id, &V0_1).await?;
        let build = Build::start(&mut conn, release_id).await?;
        let id = build.id();
        let OpenBuild::InProgress(stale) = Build::open(&mut conn, id).await? else {
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
        let OpenBuild::Finished(finished) = Build::open(&mut conn, id).await? else {
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
        let early = Build::start(&mut conn, release_id)
            .await?
            .fail_early()
            .save(&mut conn)
            .await?;
        assert!(matches!(
            Build::open(&mut conn, early.id()).await?,
            OpenBuild::EarlyFailure(_)
        ));
        let finished = Build::start(&mut conn, release_id)
            .await?
            .finish()
            .rustc_version("rustc 1.84.0-nightly (e7c0d2750 2024-10-15)")
            .docsrs_version("docs.rs test")
            .successful(false)
            .save(&mut conn)
            .await?;
        assert!(matches!(
            Build::open(&mut conn, finished.id()).await?,
            OpenBuild::Finished(_)
        ));
        Ok(())
    }
}

#[derive(Debug, Default)]
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

#[derive(Debug)]
pub struct InProgress;

#[derive(Debug)]
pub struct Finished {
    pub status: BuildStatus,
}

#[derive(Debug)]
pub struct EarlyFailure;

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

#[derive(Debug)]
pub struct Build<State> {
    id: BuildId,
    state: State,
}

/// Loading a database row requires inspecting its state at runtime.
#[derive(Debug)]
pub enum OpenBuild {
    InProgress(Build<InProgress>),
    Finished(Build<Finished>),
    EarlyFailure(Build<EarlyFailure>),
}

impl<State> Build<State> {
    pub fn id(&self) -> BuildId {
        self.id
    }

    pub fn state(&self) -> &State {
        &self.state
    }
}

#[bon::bon]
impl Build<InProgress> {
    /// Upload a target log, then register it in the database. Missing logs
    /// are reported and omitted; failed uploads never create a log record.
    #[builder(finish_fn = save)]
    pub async fn publish_build_log(
        &self,
        #[builder(finish_fn)] conn: &mut sqlx::PgConnection,
        #[builder(finish_fn)] storage: &docs_rs_storage::AsyncStorage,
        #[builder(into)] target: String,
        #[builder(default)] kind: BuildLogKind,
        #[builder(into)] log: String,
        successful: bool,
    ) -> Result<()> {
        let filename = format!("{target}{}", kind.suffix());
        storage
            .store_one(format!("build-logs/{}/{filename}", self.id), log.to_owned())
            .await?;

        let logs_filename = [filename];
        let successes = [successful];
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
        Ok(())
    }

    /// Start a new attempt, marking any previous in-progress attempts aborted.
    pub async fn start(conn: &mut sqlx::PgConnection, release_id: ReleaseId) -> Result<Self> {
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

        transaction.commit().await?;
        Ok(Self {
            id: build_id,
            state: InProgress,
        })
    }

    /// Open an existing attempt without creating a new one or changing its state.
    ///
    /// ```ignore
    /// match Build::open(conn, build_id).await? {
    ///     OpenBuild::InProgress(build) => {
    ///         let finished = build.finish()
    ///             .rustc_version("rustc 1.84.0-nightly (000000000 2024-10-01)")
    ///             .docsrs_version("docs.rs 1.0.0")
    ///             .successful(false)
    ///             .error(&error)
    ///             .save(conn)
    ///             .await?;
    ///     }
    ///     OpenBuild::Finished(_) | OpenBuild::EarlyFailure(_) => {}
    /// }
    /// ```
    pub async fn open(conn: &mut sqlx::PgConnection, id: BuildId) -> Result<OpenBuild> {
        let (status, finished) = sqlx::query_as::<_, (BuildStatus, bool)>(
            "SELECT build_status, build_finished IS NOT NULL FROM builds WHERE id = $1",
        )
        .bind(id.0)
        .fetch_one(conn)
        .await?;

        Ok(match (status, finished) {
            (BuildStatus::InProgress, false) => OpenBuild::InProgress(Self {
                id,
                state: InProgress,
            }),
            (BuildStatus::Failure, false) => OpenBuild::EarlyFailure(Build {
                id,
                state: EarlyFailure,
            }),
            (status, true) if status != BuildStatus::InProgress => OpenBuild::Finished(Build {
                id,
                state: Finished { status },
            }),
            _ => return Err(anyhow!("build {id} has inconsistent lifecycle fields")),
        })
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

        transaction.commit().await?;
        Ok(Build {
            id: self.id,
            state: Finished { status },
        })
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

        transaction.commit().await?;
        Ok(Build {
            id: self.id,
            state: EarlyFailure,
        })
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
