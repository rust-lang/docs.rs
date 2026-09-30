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
                docs_rs_database::releases::initialize_build(conn, release_id).await
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
