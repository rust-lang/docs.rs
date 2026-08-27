use crate::{BuildEnvironment, BuildStepError, ReleaseBuildResult, StepResult, TargetBuildResult};
use anyhow::{Context as _, Result, bail};
use bon::bon;
use docs_rs_build_limits::Limits;
use docs_rs_cargo_metadata::CargoMetadata;
use docs_rs_types::doc_coverage::{self, DocCoverage};
use docsrs_metadata::{BuildTargets, DEFAULT_TARGETS, Metadata};
use rustwide::Build;
use rustwide::{
    cmd::{Command, CommandError, ProcessLinesActions, ProcessOutput},
    logging::{self, LogStorage},
};
use std::{
    cell::RefCell,
    collections::HashSet,
    ffi::{OsStr, OsString},
    fs::{self, File},
    io::{BufRead as _, BufReader},
    iter,
    path::{Path, PathBuf},
    time::Duration,
};
use tracing::warn;

/// Options that vary between invocations of the docs.rs Cargo command.
#[derive(Clone, Debug, Default)]
pub struct CommandOptions {
    /// Extra arguments passed to Cargo before rustdoc's argument separator.
    pub cargo_args: Vec<String>,
    /// Extra rustdoc flags, such as the HTML or JSON output mode.
    pub rustdoc_args: Vec<String>,
}

/// A fully prepared docs.rs Cargo command backed by rustwide.
#[derive(Debug)]
#[must_use = "call `.run()` or `.run_capture()` to execute the command"]
pub struct DocsRsCommand<'ws, 'pl> {
    inner: Command<'ws, 'pl>,
}

impl<'ws, 'pl> DocsRsCommand<'ws, 'pl> {
    pub(crate) fn new(inner: Command<'ws, 'pl>) -> Self {
        Self { inner }
    }

    /// Add a raw argument to the already prepared command.
    ///
    /// Prefer [`CommandOptions::cargo_args`] or
    /// [`CommandOptions::rustdoc_args`] when argument placement matters.
    pub fn arg(mut self, arg: impl Into<OsString>) -> Self {
        self.inner = self.inner.arg(arg);
        self
    }

    /// Add raw arguments to the already prepared command.
    pub fn args<S: Into<OsString>>(mut self, args: impl IntoIterator<Item = S>) -> Self {
        self.inner = self.inner.args(args);
        self
    }

    /// Add an environment variable.
    pub fn env(mut self, key: impl Into<OsString>, value: impl Into<OsString>) -> Self {
        self.inner = self.inner.env(key, value);
        self
    }

    /// Override the command's working directory.
    pub fn current_directory(mut self, path: impl Into<PathBuf>) -> Self {
        self.inner = self.inner.current_directory(path);
        self
    }

    /// Override the command timeout.
    pub fn timeout(mut self, timeout: Option<Duration>) -> Self {
        self.inner = self.inner.timeout(timeout);
        self
    }

    /// Override the command's no-output timeout.
    pub fn no_output_timeout(mut self, timeout: Option<Duration>) -> Self {
        self.inner = self.inner.no_output_timeout(timeout);
        self
    }

    /// Enable or disable command output logging.
    pub fn log_output(mut self, enabled: bool) -> Self {
        self.inner = self.inner.log_output(enabled);
        self
    }

    /// Enable or disable command-line logging.
    pub fn log_command(mut self, enabled: bool) -> Self {
        self.inner = self.inner.log_command(enabled);
        self
    }

    /// Execute the command.
    pub fn run(self) -> Result<(), CommandError> {
        self.inner.run()
    }

    /// Execute the command and capture its output.
    pub fn run_capture(self) -> Result<ProcessOutput, CommandError> {
        self.inner.run_capture()
    }

    /// Unwrap the prepared rustwide command for APIs not forwarded here.
    pub fn into_inner(self) -> Command<'ws, 'pl> {
        self.inner
    }
}

impl<'ws> DocsRsCommand<'ws, 'static> {
    /// Process each stdout and stderr line while the command runs.
    pub fn process_lines<'pl>(
        self,
        callback: &'pl mut dyn FnMut(&str, &mut ProcessLinesActions),
    ) -> DocsRsCommand<'ws, 'pl> {
        DocsRsCommand::new(self.inner.process_lines(callback))
    }
}
