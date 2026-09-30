use crate::{
    errored_build::{FakeEarlyErrorBuild, FakeEarlyErrorBuildBuilder},
    finished_build::{FakeFinishedBuild, FakeFinishedBuildBuilder},
};
use anyhow::Result;
use docs_rs_storage::AsyncStorage;
use docs_rs_types::{BuildId, ReleaseId};

/// A build fixture at one of the lifecycle states supported by the database.
pub struct FakeBuild(BuildState);

enum BuildState {
    InProgress,
    Finished(FakeFinishedBuild),
    EarlyFailure(FakeEarlyErrorBuild),
}

impl Default for FakeBuild {
    fn default() -> Self {
        FakeFinishedBuild::default().into()
    }
}

impl From<FakeFinishedBuild> for FakeBuild {
    fn from(build: FakeFinishedBuild) -> Self {
        Self(BuildState::Finished(build))
    }
}

impl From<FakeEarlyErrorBuild> for FakeBuild {
    fn from(build: FakeEarlyErrorBuild) -> Self {
        Self(BuildState::EarlyFailure(build))
    }
}

impl FakeBuild {
    /// Create a build fixture that has started but has not completed.
    pub fn in_progress() -> Self {
        Self(BuildState::InProgress)
    }

    /// Configure a build that failed before compiler metadata was available.
    pub fn early_error() -> FakeEarlyErrorBuildBuilder {
        FakeEarlyErrorBuild::builder()
    }

    /// Configure a finished build fixture.
    pub fn finished() -> FakeFinishedBuildBuilder {
        FakeFinishedBuild::builder()
    }

    pub async fn create(
        &self,
        conn: &mut sqlx::PgConnection,
        storage: &AsyncStorage,
        release_id: ReleaseId,
        default_target: &str,
    ) -> Result<BuildId> {
        match &self.0 {
            BuildState::InProgress => {
                Ok(docs_rs_database::build::AnyBuild::start(conn, release_id)
                    .await?
                    .id())
            }
            BuildState::Finished(build) => {
                build
                    .create(conn, storage, release_id, default_target)
                    .await
            }
            BuildState::EarlyFailure(build) => build.create(conn, release_id).await,
        }
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
        for build in [
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
        ] {
            build
                .create(&mut conn, &storage, release_id, "x86_64-unknown-linux-gnu")
                .await?;
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
