use crate::{ActiveReleaseBuild, BuildEnvironment, ReleaseBuildResult};
use anyhow::Context as _;
use docs_rs_build_limits::Limits;
use rustwide::{BuildResult, Crate, Workspace};
use std::{
    collections::hash_map::DefaultHasher,
    hash::{Hash, Hasher},
};
use tracing::warn;

/// A crate release whose build lifecycle is managed by docs.rs.
pub struct ReleaseContext<'release, 'env> {
    pub(crate) environment: &'release BuildEnvironment<'env>,
    pub(crate) krate: &'release Crate,
    pub(crate) limits: Limits,
}

impl<'release, 'env> ReleaseContext<'release, 'env> {
    /// Build coverage, rustdoc JSON, and HTML for the full docs.rs target set.
    pub fn build_all_targets(self) -> anyhow::Result<BuildResult<ReleaseBuildResult>> {
        self.run(|build| build.build_all_targets())
    }

    /// Build only targets explicitly selected by the crate's docs.rs metadata.
    pub fn build_configured_targets(self) -> anyhow::Result<BuildResult<ReleaseBuildResult>> {
        self.run(|build| build.build_configured_targets())
    }

    /// Run selected build operations in one reusable sandbox.
    pub fn run<R>(
        self,
        callback: impl for<'build, 'ws> FnOnce(
            ActiveReleaseBuild<'build, 'env, 'ws>,
        ) -> anyhow::Result<R>,
    ) -> anyhow::Result<BuildResult<R>> {
        let Self {
            environment,
            krate,
            limits,
        } = self;
        environment.workspace().purge_all_build_dirs()?;
        krate.fetch(environment.workspace())?;
        let mut build_dir = environment.workspace().build_dir(&build_dir_name(krate));
        let sandbox = environment.sandbox(&limits);
        let result = build_dir
            .build(environment.toolchain(), krate, sandbox)
            .run(|build| callback(ActiveReleaseBuild::new(environment, build, limits)?));
        finish_cached_build(environment.workspace(), krate, result)
    }
}

fn build_dir_name(krate: &Crate) -> String {
    let mut hasher = DefaultHasher::new();
    krate.to_string().hash(&mut hasher);
    format!("release-{:016x}", hasher.finish())
}

fn finish_cached_build<T>(
    workspace: &Workspace,
    krate: &Crate,
    result: anyhow::Result<T>,
) -> anyhow::Result<T> {
    let purge = krate
        .purge_from_cache(workspace)
        .context("purging the crate from rustwide's cache");
    match (result, purge) {
        (Ok(output), Ok(())) => Ok(output),
        (Ok(_), Err(error)) => Err(error),
        (Err(error), Ok(())) => Err(error),
        (Err(build_error), Err(purge_error)) => {
            warn!(?purge_error, "failed to purge crate after failed build");
            Err(build_error)
        }
    }
}
