use crate::{BuildEnvironment, ReleaseBuild};
use anyhow::Result;
use docs_rs_build_limits::Limits;
use rustwide::{BuildResult, Crate};
use std::{
    collections::hash_map::DefaultHasher,
    hash::{Hash, Hasher},
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
        environment.validate_host_resources(limits)?;
        environment.workspace().purge_all_build_dirs()?;
        krate.fetch(environment.workspace())?;
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
