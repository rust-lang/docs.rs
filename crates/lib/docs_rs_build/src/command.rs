use anyhow::Result;
use docs_rs_build_limits::Limits;
use docsrs_metadata::{BuildTargets, DEFAULT_TARGETS, Metadata};
use rustwide::{Build, cmd::Command};
use std::path::PathBuf;

use crate::BuildEnvironment;

/// Name of rustdoc's documentation output directory.
pub const DOC_OUTPUT_DIR_NAME: &str = "doc";

const UNCONDITIONAL_RUSTDOC_ARGS: &[&str] = &[
    "--static-root-path",
    "/-/rustdoc.static/",
    "--cap-lints",
    "warn",
    "--extern-html-root-takes-precedence",
];

/// Options that vary between invocations of the docs.rs Cargo command.
#[derive(Clone, Debug, Default)]
pub struct CommandOptions {
    /// Extra rustdoc flags, such as the HTML or JSON output mode.
    pub rustdoc_args: Vec<String>,
}

/// One prepared crate release inside an active rustwide build.
///
/// This binds metadata and limits once so every target and output-mode command
/// for the release uses the same configuration.
pub struct ReleaseContext<'build, 'env, 'ws> {
    environment: &'build BuildEnvironment<'env>,
    build: &'build Build<'ws>,
    metadata: Metadata,
    limits: Limits,
}

impl<'build, 'env, 'ws> ReleaseContext<'build, 'env, 'ws> {
    pub(crate) fn new(
        environment: &'build BuildEnvironment<'env>,
        build: &'build Build<'ws>,
        limits: Limits,
    ) -> Result<Self> {
        let metadata = Metadata::from_crate_root(build.host_source_dir())?;

        Ok(Self {
            environment,
            build,
            metadata,
            limits,
        })
    }

    /// Prepare the Cargo command used by docs.rs for one documentation target.
    ///
    /// The command runs inside this build's sandbox. Dependencies must be
    /// fetched beforehand because docs.rs invokes Cargo in offline mode.
    pub fn command<'pl>(&self, target: &str, options: CommandOptions) -> Result<Command<'ws, 'pl>> {
        let cargo_args = cargo_args(
            target,
            &self.metadata,
            self.environment.cargo_jobs(),
            options,
        );

        if !DEFAULT_TARGETS.contains(&target) && !uses_build_std(&cargo_args) {
            self.environment
                .toolchain()
                .add_target(self.environment.workspace(), target)?;
        }

        let mut command = self
            .build
            .cargo()
            .timeout(Some(self.limits.timeout()))
            .no_output_timeout(None);

        for (key, value) in self.metadata.environment_variables() {
            command = command.env(key, value);
        }

        Ok(command.args(&cargo_args))
    }

    /// Return the host path containing documentation for a target.
    ///
    /// Cargo places proc-macro documentation in the host target directory even
    /// when a target argument is otherwise in use.
    pub fn output_dir(&self, target: &str) -> PathBuf {
        if self.metadata.proc_macro {
            self.build.host_target_dir().join(DOC_OUTPUT_DIR_NAME)
        } else {
            self.build
                .host_target_dir()
                .join(target)
                .join(DOC_OUTPUT_DIR_NAME)
        }
    }

    /// Targets selected by this release's docs.rs metadata.
    pub fn targets(&self, include_default_targets: bool) -> BuildTargets<'_> {
        self.metadata.targets(include_default_targets)
    }

    /// Fetch dependencies needed by `-Zbuild-std` before offline commands run.
    pub fn fetch_build_std_dependencies(&self, targets: &[&str]) -> Result<()> {
        self.build.fetch_build_std_dependencies(targets)
    }

    /// Metadata parsed from the prepared crate source.
    pub fn metadata(&self) -> &Metadata {
        &self.metadata
    }

    /// Limits applied to this release.
    pub fn limits(&self) -> &Limits {
        &self.limits
    }

    /// The underlying active rustwide build.
    pub fn rustwide_build(&self) -> &Build<'ws> {
        self.build
    }
}

fn cargo_args(
    target: &str,
    metadata: &Metadata,
    cargo_jobs: Option<usize>,
    mut options: CommandOptions,
) -> Vec<String> {
    let mut additional_args = vec![
        "--offline".into(),
        "-Zunstable-options".into(),
        format!(
            r#"--config=doc.extern-map.registries.crates-io="https://docs.rs/{{pkg_name}}/{{version}}/{target}""#
        ),
    ];

    if let Some(jobs) = cargo_jobs {
        additional_args.push(format!("-j{jobs}"));
    }

    // Cargo puts proc-macro documentation in the host target directory and
    // does not reliably forward RUSTDOCFLAGS when --target is supplied.
    if !metadata.proc_macro {
        additional_args.push("--target".into());
        additional_args.push(target.into());
    }

    options
        .rustdoc_args
        .extend(UNCONDITIONAL_RUSTDOC_ARGS.iter().map(|arg| (*arg).into()));
    metadata.cargo_args(&additional_args, &options.rustdoc_args)
}

fn uses_build_std(args: &[String]) -> bool {
    args.iter().enumerate().any(|(index, arg)| {
        arg.starts_with("-Zbuild-std")
            || (arg == "-Z"
                && args
                    .get(index + 1)
                    .is_some_and(|next| next.starts_with("build-std")))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognizes_build_std_spellings() {
        assert!(uses_build_std(&["-Zbuild-std=core".into()]));
        assert!(uses_build_std(&["-Z".into(), "build-std".into()]));
        assert!(!uses_build_std(&["-Zunstable-options".into()]));
    }
}
