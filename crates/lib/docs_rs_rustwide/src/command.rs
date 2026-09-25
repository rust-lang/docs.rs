use crate::{args::CommandArgs, build::ReleaseBuild, utils::args_contain_unstable_feature};
use anyhow::{Context as _, Result};
use rustwide::cmd::Command;
use std::iter;
use tracing::{debug, instrument};

/// Collect arguments and prepare a sandboxed documentation command.
///
/// Argument construction lives in `CommandArgs`. Preparation installs the target
/// or fetches build-std dependencies before creating the rustwide command.
#[must_use = "call `.prepare()` to create and prepare the rustwide command"]
pub struct PrepareCommand<'release_build, 'build, 'ws> {
    release_build: &'release_build ReleaseBuild<'build, 'ws>,

    args: CommandArgs<'release_build>,
}

impl<'release_build, 'build, 'ws> PrepareCommand<'release_build, 'build, 'ws> {
    pub(crate) fn new(
        release_build: &'release_build ReleaseBuild<'build, 'ws>,
        target: impl Into<String>,
    ) -> Self {
        Self {
            release_build,
            args: CommandArgs::new(
                &release_build.docsrs_metadata,
                target,
                release_build.environment.cargo_jobs(),
                release_build.environment.rustdoc_lints(),
            ),
        }
    }

    pub fn cargo_arg(mut self, arg: impl Into<String>) -> Self {
        self.args = self.args.cargo_arg(arg);
        self
    }

    pub fn cargo_args<S: Into<String>>(mut self, args: impl IntoIterator<Item = S>) -> Self {
        self.args = self.args.cargo_args(args);
        self
    }

    pub fn rustdoc_arg(mut self, arg: impl Into<String>) -> Self {
        self.args = self.args.rustdoc_arg(arg);
        self
    }

    pub fn rustdoc_args<S: Into<String>>(mut self, args: impl IntoIterator<Item = S>) -> Self {
        self.args = self.args.rustdoc_args(args);
        self
    }

    #[instrument(skip_all)]
    pub fn prepare<'pl>(self) -> Result<Command<'ws, 'pl>> {
        let target = self.args.target();
        debug!(target, "preparing Cargo command");
        let cargo_args = self.args.finish();

        let uses_build_std = args_contain_unstable_feature(&cargo_args, "build-std");
        if uses_build_std {
            debug!("fetching build-std dependencies for command");
            self.release_build
                .fetch_build_std_dependencies(iter::once(target))
                .context("error fetching build_std dependencies")?;
        } else {
            debug!(target, "ensuring command target is installed");
            self.release_build
                .environment
                .ensure_target_installed(target)?;
        }

        debug!(
            uses_build_std,
            argument_count = cargo_args.len(),
            "Cargo command prepared"
        );
        Ok(self.release_build.build_rustwide_command().args(cargo_args))
    }
}
