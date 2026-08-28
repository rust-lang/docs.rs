use anyhow::Result;
use docsrs_metadata::{BuildTargets, DEFAULT_TARGETS, Metadata};
use rustwide::cmd::{Command, CommandError, ProcessLinesActions, ProcessOutput};
use std::{path::PathBuf, time::Duration};

const UNCONDITIONAL_RUSTDOC_ARGS: &[&str] = &[
    "--static-root-path",
    "/-/rustdoc.static/",
    "--cap-lints",
    "warn",
    "--extern-html-root-takes-precedence",
];

/// A fully prepared docs.rs Cargo command backed by rustwide.
#[derive(Debug)]
#[must_use = "call `.run()` or `.run_capture()` to execute the command"]
pub struct DocsRsCommand<'ws, 'pl> {
    inner: Command<'ws, 'pl>,

    target: String,

    /// Extra arguments passed to Cargo before rustdoc's argument separator.
    cargo_args: Vec<String>,

    /// Extra rustdoc flags, such as the HTML or JSON output mode.
    rustdoc_args: Vec<String>,
}

impl<'ws, 'pl> DocsRsCommand<'ws, 'pl> {
    pub(crate) fn new(inner: Command<'ws, 'pl>, target: impl Into<String>) -> Self {
        Self {
            inner,
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

    // /// Add an environment variable.
    // pub fn env(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
    //     self.inner = self.inner.env(key, value);
    //     self
    // }

    // /// Override the command's working directory.
    // pub fn current_directory(mut self, path: impl Into<PathBuf>) -> Self {
    //     self.inner = self.inner.current_directory(path);
    //     self
    // }

    // /// Override the command timeout.
    // pub fn timeout(mut self, timeout: Option<Duration>) -> Self {
    //     self.inner = self.inner.timeout(timeout);
    //     self
    // }

    // /// Override the command's no-output timeout.
    // pub fn no_output_timeout(mut self, timeout: Option<Duration>) -> Self {
    //     self.inner = self.inner.no_output_timeout(timeout);
    //     self
    // }

    /// Enable or disable command output logging.
    pub fn log_output(mut self, enabled: bool) -> Self {
        self.inner = self.inner.log_output(enabled);
        self
    }

    // /// Enable or disable command-line logging.
    // pub fn log_command(mut self, enabled: bool) -> Self {
    //     self.inner = self.inner.log_command(enabled);
    //     self
    // }

    /// Execute the command.
    pub fn run(self) -> Result<(), CommandError> {
        // if uses_build_std(&cargo_args) {
        //     self.fetch_build_std_dependencies([target])?;
        // } else if !DEFAULT_TARGETS.contains(&target) {
        //     self.environment
        //         .configured_toolchain()
        //         .add_target(self.environment.workspace(), target)?;
        // }
        self.inner.run()
    }

    /// Execute the command and capture its output.
    pub fn run_capture(self) -> Result<ProcessOutput, CommandError> {
        self.inner.run_capture()
    }

    // /// Unwrap the prepared rustwide command for APIs not forwarded here.
    // pub fn into_inner(self) -> Command<'ws, 'pl> {
    //     self.inner
    // }
}

impl<'ws> DocsRsCommand<'ws, 'static> {
    /// Process each stdout and stderr line while the command runs.
    pub fn process_lines<'pl>(
        self,
        callback: &'pl mut dyn FnMut(&str, &mut ProcessLinesActions),
    ) -> DocsRsCommand<'ws, 'pl> {
        todo!();
        // self.inner = self.inner.process_lines(callback);
        // self
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
        ).into(),
    ];

    if let Some(jobs) = cargo_jobs {
        additional_args.push(format!("-j{jobs}").into());
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
