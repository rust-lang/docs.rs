use crate::BuildEnvironment;
use anyhow::{Context as _, Result};
use docs_rs_build_limits::Limits;
use docsrs_metadata::Metadata;
use rustwide::{Build, BuildResult, Crate, Workspace};
use std::{
    collections::hash_map::DefaultHasher,
    hash::{Hash, Hasher},
};
use tracing::warn;

/// A crate release whose build lifecycle is managed by docs.rs.
pub struct ReleaseContext<'release> {
    pub(crate) environment: &'release BuildEnvironment,
    pub(crate) krate: &'release Crate,
    pub(crate) limits: Option<Limits>,
}

impl<'release> ReleaseContext<'release> {
    /// Override the environment's default limits for this release.
    pub fn limits(mut self, limits: Limits) -> Self {
        self.limits = Some(limits);
        self
    }

    /// Run selected build operations in one reusable sandbox.
    pub fn run<R>(
        self,
        callback: impl for<'build, 'ws> FnOnce(ActiveReleaseBuild<'build, 'ws>) -> Result<R>,
    ) -> Result<BuildResult<R>> {
        let Self {
            environment,
            krate,
            limits,
        } = self;
        environment.workspace().purge_all_build_dirs()?;
        krate.fetch(environment.workspace())?;
        let mut build_dir = environment.workspace().build_dir(&build_dir_name(krate));
        let limits = limits.as_ref().unwrap_or(self.environment.default_limits());
        let sandbox = environment.sandbox(&limits);
        let result = build_dir
            .build(environment.configured_toolchain(), krate, sandbox)
            .run(|build| callback(ActiveReleaseBuild::new(environment, build, limits)?));
        finish_cached_build(environment.workspace(), krate, result)
    }
}

/// A prepared release inside an active rustwide sandbox.
pub struct ActiveReleaseBuild<'build, 'ws> {
    pub(crate) environment: &'build BuildEnvironment,
    pub(crate) build: &'build Build<'ws>,
    pub(crate) metadata: Metadata,
    pub(crate) limits: &'build Limits,
    pub(crate) resource_suffix: String,
}

impl<'build, 'ws> ActiveReleaseBuild<'build, 'ws> {
    pub(crate) fn new(
        environment: &'build BuildEnvironment,
        build: &'build Build<'ws>,
        limits: &'build Limits,
    ) -> Result<Self> {
        let metadata = Metadata::from_crate_root(build.host_source_dir())?;
        let resource_suffix = environment.resource_suffix()?;

        Ok(Self {
            environment,
            build,
            metadata,
            limits,
            resource_suffix,
        })
    }
}

fn build_dir_name(krate: &Crate) -> String {
    let mut hasher = DefaultHasher::new();
    krate.to_string().hash(&mut hasher);
    format!("release-{:016x}", hasher.finish())
}

fn finish_cached_build<T>(workspace: &Workspace, krate: &Crate, result: Result<T>) -> Result<T> {
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
