//! Typed build handles and lifecycle transitions.
//!
//! A transition consumes its in-progress handle and checks the database state.

use crate::releases::{add_build_logs, finish_build, initialize_build, update_build_with_error};
use anyhow::{Result, anyhow};
use docs_rs_types::{BuildError, BuildId, BuildStatus, ByteSize, ReleaseId};

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
        add_build_logs(conn, self.id, [(filename, successful)]).await
    }

    /// Start a new attempt, marking any previous in-progress attempts aborted.
    pub async fn start(conn: &mut sqlx::PgConnection, release_id: ReleaseId) -> Result<Self> {
        let mut transaction = sqlx::Connection::begin(conn).await?;
        let id = initialize_build(&mut transaction, release_id).await?;
        transaction.commit().await?;
        Ok(Self {
            id,
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
        finish_build(
            &mut transaction,
            self.id,
            rustc_version,
            docsrs_version,
            status,
            documentation_size,
            memory_peak,
            error.as_ref(),
        )
        .await?;
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
        update_build_with_error(&mut transaction, self.id, error.as_ref()).await?;
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
