use crate::{build::ReleaseBuild, utils::args_contain_unstable_feature};
use anyhow::{Context as _, Result};
use docsrs_metadata::Metadata;
use rustwide::cmd::Command;
use tracing::{debug, instrument};

const UNCONDITIONAL_RUSTDOC_ARGS: &[&str] = &[
    "--static-root-path",
    "/-/rustdoc.static/",
    "--cap-lints",
    "warn",
    "--extern-html-root-takes-precedence",
];

#[must_use = "call `.prepare()` to create and prepare the rustwide command"]
pub struct PrepareCommand<'release_build, 'build, 'ws> {
    release_build: &'release_build ReleaseBuild<'build, 'ws>,

    target: String,

    /// Extra arguments passed to Cargo before rustdoc's argument separator.
    cargo_args: Vec<String>,

    /// Extra rustdoc flags, such as the HTML or JSON output mode.
    rustdoc_args: Vec<String>,
}

impl<'release_build, 'build, 'ws> PrepareCommand<'release_build, 'build, 'ws> {
    pub(crate) fn new(
        release_build: &'release_build ReleaseBuild<'build, 'ws>,
        target: impl Into<String>,
    ) -> Self {
        Self {
            release_build,
            target: target.into(),
            cargo_args: Vec::new(),
            rustdoc_args: Vec::new(),
        }
    }

    pub fn cargo_arg(mut self, arg: impl Into<String>) -> Self {
        self.cargo_args.push(arg.into());
        self
    }

    pub fn cargo_args<S: Into<String>>(mut self, args: impl IntoIterator<Item = S>) -> Self {
        self.cargo_args.extend(args.into_iter().map(Into::into));
        self
    }

    pub fn rustdoc_arg(mut self, arg: impl Into<String>) -> Self {
        self.rustdoc_args.push(arg.into());
        self
    }

    pub fn rustdoc_args<S: Into<String>>(mut self, args: impl IntoIterator<Item = S>) -> Self {
        self.rustdoc_args.extend(args.into_iter().map(Into::into));
        self
    }

    #[instrument(skip_all, fields(target = %self.target))]
    pub fn prepare<'pl>(self) -> Result<Command<'ws, 'pl>> {
        debug!(
            cargo_arg_count = self.cargo_args.len(),
            rustdoc_arg_count = self.rustdoc_args.len(),
            "preparing Cargo command"
        );
        let cargo_args = cargo_args(
            &self.target,
            &self.release_build.metadata,
            self.release_build.environment.cargo_jobs(),
            self.cargo_args,
            self.rustdoc_args,
        );

        let uses_build_std = args_contain_unstable_feature(&cargo_args, "build-std");
        if uses_build_std {
            debug!("fetching build-std dependencies for command");
            self.release_build
                .fetch_build_std_dependencies([self.target.as_ref()])
                .context("error fetching build_std dependencies")?;
        } else {
            debug!("ensuring command target is installed");
            self.release_build
                .environment
                .ensure_target_installed(&self.target)?;
        }

        debug!(
            uses_build_std,
            argument_count = cargo_args.len(),
            "Cargo command prepared"
        );
        Ok(self.release_build.build_rustwide_command().args(cargo_args))
    }
}

fn cargo_args(
    target: &str,
    metadata: &Metadata,
    cargo_jobs: Option<usize>,
    cargo_args: Vec<String>,
    mut rustdoc_args: Vec<String>,
) -> Vec<String> {
    let mut additional_args: Vec<String> = vec![
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

    additional_args.extend(cargo_args);

    rustdoc_args.extend(UNCONDITIONAL_RUSTDOC_ARGS.iter().map(|arg| (*arg).into()));
    metadata.cargo_args(&additional_args, &rustdoc_args)
}
