use crate::{BuildEnvironment, BuildResult, ReleaseBuild};
use anyhow::Result;
use docs_rs_build_limits::Limits;
use rustwide::Crate;
use std::{
    collections::hash_map::DefaultHasher,
    hash::{Hash, Hasher},
    path::Path,
    time::Instant,
};
use tracing::{debug, info, instrument};

/// A release that has not yet been fetched into the workspace cache.
pub struct Unfetched;

/// A fetched release, carrying the start of its build lifecycle.
pub struct Fetched {
    started: Instant,
}

/// A crate release whose build lifecycle is managed by docs.rs.
///
/// Fetching transitions from [`Unfetched`] to [`Fetched`], enabling source copying.
pub struct ReleaseContext<'release, State = Unfetched> {
    pub(crate) environment: &'release mut BuildEnvironment,
    pub(crate) krate: &'release Crate,
    pub(crate) limits: Option<Limits>,
    pub(crate) directory_label: Option<String>,
    pub(crate) state: State,
}

impl<State> ReleaseContext<'_, State> {
    /// Add a human-readable label to the build directory, such as `headers-0.4.1`.
    ///
    /// Characters other than ASCII letters, digits, dots, hyphens, and underscores
    /// are replaced with underscores. A crate hash is always appended; an empty
    /// label uses the default `release` prefix. This does not change package selection.
    pub fn directory_label(mut self, label: impl Into<String>) -> Self {
        self.directory_label = Some(label.into());
        self
    }

    /// Override the environment's default limits for this release.
    pub fn limits(mut self, limits: Limits) -> Self {
        self.limits = Some(limits);
        self
    }
}

impl<'release> ReleaseContext<'release, Unfetched> {
    /// Fetch this release into rustwide's crate cache.
    ///
    /// The returned phase allows callers to archive the fetched sources before
    /// metadata parsing or sandbox preparation can fail.
    #[instrument(skip_all)]
    pub fn fetch(self) -> Result<ReleaseContext<'release, Fetched>> {
        let started = Instant::now();
        let Self {
            environment,
            krate,
            limits,
            directory_label,
            state: Unfetched,
        } = self;

        info!(%krate, "fetching crate source");
        krate.fetch(environment.workspace())?;
        debug!("crate source fetched");

        Ok(ReleaseContext {
            state: Fetched { started },
            environment,
            krate,
            limits,
            directory_label,
        })
    }

    /// Fetch the release and run selected build operations in one reusable sandbox.
    ///
    /// Shortcut for:
    ///
    /// ```no_run
    /// # use anyhow::Result;
    /// # use docs_rs_rustwide::BuildEnvironment;
    /// # use rustwide::Crate;
    /// # fn example(environment: &mut BuildEnvironment, krate: &Crate) -> Result<()> {
    /// let result = environment
    ///     .release(krate)
    ///     .fetch()?
    ///     .run(|build| Ok(build.build_docs()))?;
    /// # Ok(())
    /// # }
    /// ```
    pub fn run<R>(
        self,
        callback: impl for<'build, 'ws> FnOnce(ReleaseBuild<'build, 'ws>) -> Result<R>,
    ) -> Result<BuildResult<R>> {
        self.fetch()?.run(callback)
    }
}

impl ReleaseContext<'_, Fetched> {
    /// Copy the fetched crate sources into a caller-owned directory.
    ///
    /// This is intended for source archiving before the build sandbox is entered.
    #[instrument(skip_all)]
    pub fn copy_source_to(&self, destination: impl AsRef<Path>) -> Result<()> {
        info!(
            krate = %self.krate,
            destination = %destination.as_ref().display(),
            "copying fetched crate source"
        );

        self.krate
            .copy_source_to(self.environment.workspace(), destination.as_ref())?;
        debug!("fetched crate source copied");
        Ok(())
    }

    /// Run selected build operations in one reusable sandbox.
    #[instrument(skip_all)]
    pub fn run<R>(
        self,
        callback: impl for<'build, 'ws> FnOnce(ReleaseBuild<'build, 'ws>) -> Result<R>,
    ) -> Result<BuildResult<R>> {
        let Self {
            state: Fetched { started },
            environment,
            krate,
            limits,
            directory_label,
        } = self;

        let effective_limits = limits.unwrap_or_else(|| environment.default_limits().clone());
        environment.validate_host_resources(&effective_limits)?;

        debug!("purging stale release build directories");
        environment.workspace().purge_all_build_dirs()?;

        let build_dir_name = build_dir_name(krate, directory_label.as_deref());
        debug!(build_dir_name, "preparing release build directory");
        let mut build_dir = environment.workspace().build_dir(&build_dir_name);

        debug!("starting release sandbox, calling callback");
        let sandbox_builder = environment.sandbox_builder(&effective_limits);
        let result = build_dir
            .build(environment.configured_toolchain(), krate, sandbox_builder)
            .run(|build| callback(ReleaseBuild::new(environment, build, &effective_limits)?))?;

        debug!("release sandbox completed; purging crate source cache");
        krate.purge_from_cache(environment.workspace())?;

        debug!("release build completed");
        Ok(BuildResult {
            inner: result,
            duration: started.elapsed().into(),
        })
    }
}

fn build_dir_name(krate: &Crate, label: Option<&str>) -> String {
    let mut hasher = DefaultHasher::new();
    krate.to_string().hash(&mut hasher);
    let label: String = label
        .unwrap_or("release")
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_') {
                c
            } else {
                '_'
            }
        })
        .take(128)
        .collect();
    let label = if label.is_empty() { "release" } else { &label };
    format!("{label}-{:016x}", hasher.finish())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn directory_labels_are_safe_and_preserve_the_hash() {
        let krate = Crate::crates_io("example", "1.0.0");
        let fallback = build_dir_name(&krate, None);
        let hash = fallback.strip_prefix("release-").unwrap();
        assert_eq!(
            build_dir_name(&krate, Some("headers-0.4.1")),
            format!("headers-0.4.1-{hash}")
        );
        assert_eq!(build_dir_name(&krate, Some("")), fallback);
        let unsafe_label = build_dir_name(&krate, Some("../../test/name\\version"));
        assert_eq!(Path::new(&unsafe_label).components().count(), 1);
        assert!(unsafe_label.ends_with(hash));
    }
}
