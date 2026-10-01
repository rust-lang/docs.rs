use crate::errored_build::StoredBuildError;
use anyhow::{Result, bail};
use docs_rs_database::build::AnyBuild;
use docs_rs_storage::AsyncStorage;
use docs_rs_types::{ByteSize, ReleaseId};
use std::collections::HashMap;

/// A deferred build fixture, persisted through the typed database API.
#[derive(bon::Builder)]
#[builder(start_fn(vis = "pub(crate)"))]
pub struct FakeBuild {
    #[builder(field)]
    lifecycle: Lifecycle,
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

use fake_build_builder::{IsComplete, IsUnset, SetS3BuildLog, State};

#[derive(Default)]
enum Lifecycle {
    InProgress,
    #[default]
    Finished,
    EarlyFailure,
}

#[bon::bon]
impl FakeBuild {
    pub fn finished() -> FakeBuildBuilder {
        Self::builder()
    }

    pub fn in_progress() -> Self {
        Self {
            lifecycle: Lifecycle::InProgress,
            ..Self::default()
        }
    }

    #[builder(finish_fn = build)]
    pub fn early_error(
        #[builder(with = |error: impl docs_rs_types::BuildError| StoredBuildError::new(error))]
        error: Option<StoredBuildError>,
    ) -> Self {
        Self {
            lifecycle: Lifecycle::EarlyFailure,
            error,
            ..Self::default()
        }
    }
}

impl<S: fake_build_early_error_builder::State> FakeBuildEarlyErrorBuilder<S> {
    pub async fn create(
        self,
        conn: &mut sqlx::PgConnection,
        release_id: ReleaseId,
    ) -> Result<AnyBuild>
    where
        S: fake_build_early_error_builder::IsComplete,
    {
        let fixture = self.build();
        let build = AnyBuild::start(conn, release_id).await?;
        Ok(AnyBuild::EarlyFailure(
            build
                .fail_early()
                .maybe_error(fixture.error.as_ref())
                .save(conn)
                .await?,
        ))
    }
}

impl<S: State> FakeBuildBuilder<S> {
    pub fn no_s3_build_log(self) -> FakeBuildBuilder<SetS3BuildLog<S>>
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
    ) -> Result<AnyBuild>
    where
        S: IsComplete,
    {
        self.build()
            .create(conn, storage, release_id, default_target)
            .await
    }
}

impl Default for FakeBuild {
    fn default() -> Self {
        Self::builder().build()
    }
}

impl FakeBuild {
    pub async fn create(
        &self,
        conn: &mut sqlx::PgConnection,
        storage: &AsyncStorage,
        release_id: ReleaseId,
        default_target: &str,
    ) -> Result<AnyBuild> {
        let mut build = AnyBuild::start(conn, release_id).await?;
        match self.lifecycle {
            Lifecycle::InProgress => return Ok(AnyBuild::InProgress(build)),
            Lifecycle::EarlyFailure => {
                return Ok(AnyBuild::EarlyFailure(
                    build
                        .fail_early()
                        .maybe_error(self.error.as_ref())
                        .save(conn)
                        .await?,
                ));
            }
            Lifecycle::Finished => {}
        }
        let build_id = build.id();
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

        if let Some((s3_build_log, successful)) = &self.s3_build_log {
            if self.legacy_build_logs {
                storage
                    .store_one(
                        format!("{prefix}{default_target}.txt"),
                        s3_build_log.clone(),
                    )
                    .await?;
            } else {
                build
                    .publish_build_log()
                    .target(format!("{default_target}.txt"))
                    .log(s3_build_log.as_str())
                    .successful(*successful)
                    .save(conn, storage)
                    .await?;
            }
        }

        for (target, (log, successful)) in &self.other_build_logs {
            if target == default_target {
                bail!("build log for default target has to be set via `s3_build_log`");
            }
            if self.legacy_build_logs {
                storage
                    .store_one(format!("{prefix}{target}.txt"), log.clone())
                    .await?;
            } else {
                build
                    .publish_build_log()
                    .target(format!("{target}.txt"))
                    .log(log.as_str())
                    .successful(*successful)
                    .save(conn, storage)
                    .await?;
            }
        }

        let build = build
            .finish()
            .rustc_version(&self.rustc_version)
            .docsrs_version(&self.docsrs_version)
            .successful(self.successful)
            .maybe_documentation_size(self.documentation_size)
            .maybe_memory_peak(self.memory_peak)
            .maybe_error(self.error.as_ref())
            .save(conn)
            .await?;

        Ok(AnyBuild::Finished(build))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use docs_rs_config::AppConfig as _;
    use docs_rs_database::{
        Config,
        releases::{initialize_crate, initialize_release},
        testing::TestDatabase,
    };
    use docs_rs_opentelemetry::testing::TestMetrics;
    use docs_rs_storage::{StorageKind, testing::TestStorage};
    use docs_rs_types::{
        BuildStatus, SimpleBuildError,
        testing::{FOO, V0_1},
    };

    #[tokio::test(flavor = "multi_thread")]
    async fn fake_build_lifecycle_fields() -> Result<()> {
        let metrics = TestMetrics::new();
        let db = TestDatabase::new(&Config::test_config()?, metrics.provider()).await?;
        let storage = TestStorage::from_kind(StorageKind::Memory, metrics.provider()).await?;
        let mut conn = db.async_conn().await?;
        let crate_id = initialize_crate(&mut conn, &FOO).await?;
        let release_id = initialize_release(&mut conn, crate_id, &V0_1).await?;
        for (index, build) in [
            FakeBuild::finished().build(),
            FakeBuild::finished()
                .successful(false)
                .error(SimpleBuildError("finished error".into()))
                .build(),
            FakeBuild::early_error()
                .error(SimpleBuildError("early error".into()))
                .build(),
            FakeBuild::early_error().build(),
            FakeBuild::in_progress(),
        ]
        .into_iter()
        .enumerate()
        {
            let build = build
                .create(&mut conn, &storage, release_id, "x86_64-unknown-linux-gnu")
                .await?;
            assert!(match index {
                0 | 1 => matches!(build, AnyBuild::Finished(_)),
                2 | 3 => matches!(build, AnyBuild::EarlyFailure(_)),
                4 => matches!(build, AnyBuild::InProgress(_)),
                _ => unreachable!(),
            });
        }
        let rows = sqlx::query_as::<
            _,
            (
                BuildStatus,
                bool,
                bool,
                bool,
                bool,
                bool,
                bool,
                Option<String>,
                Option<String>,
            ),
        >(
            "SELECT build_status, build_started IS NOT NULL, build_finished IS NOT NULL,
                    rustc_version IS NOT NULL, docsrs_version IS NOT NULL,
                    memory_peak IS NOT NULL, documentation_size IS NOT NULL, errors, error_kind
             FROM builds WHERE rid = $1 ORDER BY id",
        )
        .bind(release_id)
        .fetch_all(&mut *conn)
        .await?;
        assert_eq!(
            rows,
            vec![
                (
                    BuildStatus::Success,
                    true,
                    true,
                    true,
                    true,
                    true,
                    true,
                    None,
                    None
                ),
                (
                    BuildStatus::Failure,
                    true,
                    true,
                    true,
                    true,
                    true,
                    true,
                    Some("build error: finished error".into()),
                    Some("SimpleBuildError".into())
                ),
                (
                    BuildStatus::Failure,
                    true,
                    false,
                    false,
                    false,
                    false,
                    false,
                    Some("build error: early error".into()),
                    Some("SimpleBuildError".into())
                ),
                (
                    BuildStatus::Failure,
                    true,
                    false,
                    false,
                    false,
                    false,
                    false,
                    None,
                    None
                ),
                (
                    BuildStatus::InProgress,
                    true,
                    false,
                    false,
                    false,
                    false,
                    false,
                    None,
                    None
                ),
            ]
        );
        Ok(())
    }
}
