use crate::{FakeBuild, errored_build::StoredBuildError};
use anyhow::{Result, bail};
use docs_rs_database::releases::add_build_logs;
use docs_rs_storage::AsyncStorage;
use docs_rs_types::{BuildId, BuildStatus, ByteSize, ReleaseId};
use std::collections::HashMap;

/// A completed build with compiler metadata, metrics, and optional logs.
#[derive(bon::Builder)]
#[builder(
    start_fn(vis = "pub(crate)"),
    finish_fn(name = into_finished, vis = "pub(crate)")
)]
pub struct FakeFinishedBuild {
    #[builder(field)]
    other_build_logs: HashMap<String, (String, bool)>,

    #[builder(
        required,
        with = |build_log: impl Into<String>, successful: bool| Some((build_log.into(), successful)),
        default = Some(("It works!".into(), true))
    )]
    s3_build_log: Option<(String, bool)>,

    #[builder(into)]
    db_build_log: Option<String>,

    #[builder(into, default = "rustc 2.0.0-nightly (000000000 1970-01-01)")]
    rustc_version: String,

    #[builder(into, default = "docs.rs 1.0.0 (000000000 1970-01-01)")]
    docsrs_version: String,

    #[builder(default = true)]
    successful: bool,

    #[builder(with = |error: impl docs_rs_types::BuildError| StoredBuildError::new(error))]
    error: Option<StoredBuildError>,

    #[builder(
        required,
        with=|size: impl Into<ByteSize>| Some(size.into()),
        default = Some(ByteSize::b(23u64))
    )]
    memory_peak: Option<ByteSize>,

    #[builder(
        required,
        with=Some,
        default=Some(ByteSize::b(42u64))
    )]
    documentation_size: Option<ByteSize>,

    /// new build logs: we have a record in the `builds_logs` table for each log, including a status
    /// old build logs: people have to run `s3 ls` with prefix to know which build logs exist
    #[builder(default = false)]
    legacy_build_logs: bool,
}

use fake_finished_build_builder::{IsComplete, IsUnset, SetS3BuildLog, State};

impl<S: State> FakeFinishedBuildBuilder<S> {
    /// Finish the fixture as a build in the finished lifecycle state.
    pub fn build(self) -> FakeBuild
    where
        S: IsComplete,
    {
        self.into_finished().into()
    }

    pub fn no_s3_build_log(self) -> FakeFinishedBuildBuilder<SetS3BuildLog<S>>
    where
        S::S3BuildLog: IsUnset,
    {
        self.maybe_s3_build_log(None::<(String, bool)>)
    }

    pub fn build_log_for_other_target(
        mut self,
        target: impl Into<String>,
        build_log: impl Into<String>,
        successful: bool,
    ) -> Self {
        self.other_build_logs
            .insert(target.into(), (build_log.into(), successful));
        self
    }

    pub async fn create(
        self,
        conn: &mut sqlx::PgConnection,
        storage: &AsyncStorage,
        release_id: ReleaseId,
        default_target: &str,
    ) -> Result<BuildId>
    where
        S: IsComplete,
    {
        self.build()
            .create(conn, storage, release_id, default_target)
            .await
    }
}

impl Default for FakeFinishedBuild {
    fn default() -> Self {
        Self::builder().into_finished()
    }
}

impl FakeFinishedBuild {
    pub async fn create(
        &self,
        conn: &mut sqlx::PgConnection,
        storage: &AsyncStorage,
        release_id: ReleaseId,
        default_target: &str,
    ) -> Result<BuildId> {
        let build_id = docs_rs_database::releases::initialize_build(&mut *conn, release_id).await?;

        docs_rs_database::releases::finish_build(
            &mut *conn,
            build_id,
            &self.rustc_version,
            &self.docsrs_version,
            if self.successful {
                BuildStatus::Success
            } else {
                BuildStatus::Failure
            },
            self.documentation_size,
            self.memory_peak,
            self.error.as_ref(),
        )
        .await?;

        if let Some(db_build_log) = self.db_build_log.as_deref() {
            sqlx::query!(
                "UPDATE builds SET output = $2 WHERE id = $1",
                build_id.0,
                db_build_log
            )
            .execute(&mut *conn)
            .await?;
        }

        let prefix = format!("build-logs/{build_id}/");

        let mut log_filenames = Vec::new();

        if let Some((s3_build_log, successful)) = &self.s3_build_log {
            log_filenames.push((format!("{default_target}.txt"), *successful));
            storage
                .store_one(
                    format!("{prefix}{default_target}.txt"),
                    s3_build_log.clone(),
                )
                .await?;
        }

        for (target, (log, successful)) in &self.other_build_logs {
            if target == default_target {
                bail!("build log for default target has to be set via `s3_build_log`");
            }
            log_filenames.push((format!("{target}.txt"), *successful));
            storage
                .store_one(format!("{prefix}{target}.txt"), log.clone())
                .await?;
        }

        if !self.legacy_build_logs && !log_filenames.is_empty() {
            add_build_logs(&mut *conn, build_id, log_filenames).await?;
        }

        Ok(build_id)
    }
}
