use crate::{BuildEnvironment, ReleaseBuild};
use anyhow::Result;
use docs_rs_build_limits::Limits;
use rustwide::{BuildResult, Crate};
use std::{
    collections::hash_map::DefaultHasher,
    hash::{Hash, Hasher},
    path::Path,
};

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

    /// Fetch this release into rustwide's crate cache.
    ///
    /// The returned phase allows callers to archive the fetched sources before
    /// metadata parsing or sandbox preparation can fail.
    pub fn fetch(self) -> Result<FetchedRelease<'release>> {
        let Self {
            environment,
            krate,
            limits,
        } = self;
        let effective_limits = limits
            .as_ref()
            .unwrap_or_else(|| environment.default_limits());
        environment.validate_host_resources(effective_limits)?;
        krate.fetch(environment.workspace())?;

        Ok(FetchedRelease {
            environment,
            krate,
            limits,
        })
    }

    /// Fetch the release and run selected build operations in one reusable sandbox.
    pub fn run<R>(
        self,
        callback: impl for<'build, 'ws> FnOnce(ReleaseBuild<'build, 'ws>) -> Result<R>,
    ) -> Result<BuildResult<R>> {
        self.fetch()?.run(callback)
    }
}

/// A crate release fetched into rustwide's cache but not yet prepared for building.
pub struct FetchedRelease<'release> {
    environment: &'release BuildEnvironment,
    krate: &'release Crate,
    limits: Option<Limits>,
}

impl FetchedRelease<'_> {
    /// Copy the fetched crate sources into a caller-owned directory.
    ///
    /// This is intended for source archiving before the build sandbox is entered.
    pub fn copy_source_to(&self, destination: impl AsRef<Path>) -> Result<()> {
        self.krate
            .copy_source_to(self.environment.workspace(), destination.as_ref())?;
        Ok(())
    }

    /// Run selected build operations in one reusable sandbox.
    pub fn run<R>(
        self,
        callback: impl for<'build, 'ws> FnOnce(ReleaseBuild<'build, 'ws>) -> Result<R>,
    ) -> Result<BuildResult<R>> {
        let Self {
            environment,
            krate,
            limits,
        } = self;
        let limits = limits
            .as_ref()
            .unwrap_or_else(|| environment.default_limits());
        environment.workspace().purge_all_build_dirs()?;
        let mut build_dir = environment.workspace().build_dir(&build_dir_name(krate));
        let sandbox = environment.sandbox_builder(limits);
        let result = build_dir
            .build(environment.configured_toolchain(), krate, sandbox)
            .run(|build| callback(ReleaseBuild::new(environment, build, limits)?))?;
        krate.purge_from_cache(environment.workspace())?;
        Ok(result)
    }
}

fn build_dir_name(krate: &Crate) -> String {
    let mut hasher = DefaultHasher::new();
    krate.to_string().hash(&mut hasher);
    format!("release-{:016x}", hasher.finish())
}
