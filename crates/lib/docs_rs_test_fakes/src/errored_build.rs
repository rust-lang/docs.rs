use crate::FakeBuild;
use anyhow::Result;
use docs_rs_types::{BuildError, BuildId, ReleaseId};
use std::fmt;

/// An owned error retaining the original display text and classification.
#[derive(Debug)]
pub(crate) struct StoredBuildError {
    message: String,
    kind: &'static str,
}

impl StoredBuildError {
    pub(crate) fn new(error: impl BuildError) -> Self {
        Self {
            message: error.to_string(),
            kind: error.kind(),
        }
    }
}

impl fmt::Display for StoredBuildError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for StoredBuildError {}

impl BuildError for StoredBuildError {
    fn kind(&self) -> &'static str {
        self.kind
    }
}

/// A build that failed before compiler versions and metrics were available.
#[derive(bon::Builder)]
#[builder(
    start_fn(vis = "pub(crate)"),
    finish_fn(name = into_early_error, vis = "pub(crate)")
)]
pub struct FakeEarlyErrorBuild {
    #[builder(with = |error: impl BuildError| StoredBuildError::new(error))]
    error: Option<StoredBuildError>,
}

impl<S: fake_early_error_build_builder::State> FakeEarlyErrorBuildBuilder<S> {
    /// Finish the fixture as a build in the early failure lifecycle state.
    pub fn build(self) -> FakeBuild
    where
        S: fake_early_error_build_builder::IsComplete,
    {
        self.into_early_error().into()
    }

    pub async fn create(
        self,
        conn: &mut sqlx::PgConnection,
        release_id: ReleaseId,
    ) -> Result<BuildId>
    where
        S: fake_early_error_build_builder::IsComplete,
    {
        self.into_early_error().create(conn, release_id).await
    }
}

impl FakeEarlyErrorBuild {
    pub async fn create(
        &self,
        conn: &mut sqlx::PgConnection,
        release_id: ReleaseId,
    ) -> Result<BuildId> {
        let build = docs_rs_database::build::AnyBuild::start(conn, release_id).await?;
        Ok(build
            .fail_early()
            .maybe_error(self.error.as_ref())
            .save(conn)
            .await?
            .id())
    }
}
