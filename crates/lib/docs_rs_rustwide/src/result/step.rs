use anyhow::Result;
use docs_rs_types::BuildError;
use rustwide::cmd::CommandError;
use std::{fmt, time::Duration};

/// Failure of an individual build step.
#[derive(Debug, thiserror::Error)]
pub enum BuildStepError {
    /// Dependencies or toolchain targets could not be prepared.
    #[error(transparent)]
    Prepare(anyhow::Error),

    /// Cargo or rustdoc failed inside the sandbox.
    #[error(transparent)]
    Command(CommandError),

    /// A step's output could not be found, parsed, or collected.
    #[error(transparent)]
    Output(anyhow::Error),
}

impl BuildError for BuildStepError {
    fn kind(&self) -> &'static str {
        match self {
            Self::Command(error) => match error {
                CommandError::NoOutputFor(_) => "NoOutputFor",
                CommandError::Timeout(_) => "Timeout",
                CommandError::ExecutionFailed { .. } => "ExecutionFailed",
                CommandError::KillAfterTimeoutFailed(_) => "KillAfterTimeoutFailed",
                CommandError::SandboxOOM => "SandboxOOM",
                CommandError::SandboxImagePullFailed(_) => "SandboxImagePullFailed",
                CommandError::SandboxImageMissing(_) => "SandboxImageMissing",
                CommandError::SandboxContainerCreate(_) => "SandboxContainerCreate",
                CommandError::WorkspaceNotMountedCorrectly => "WorkspaceNotMountedCorrectly",
                CommandError::InvalidDockerInspectOutput(_) => "InvalidDockerInspectOutput",
                CommandError::IO(_) => "IO",
                _ => "UnknownCommandError",
            },
            Self::Prepare(_) => "InternalPrepare",
            Self::Output(_) => "InternalOutput",
        }
    }
}

impl BuildStepError {
    pub(crate) fn as_output<R>(
        mut f: impl FnMut() -> anyhow::Result<R>,
    ) -> Result<R, BuildStepError> {
        f().map_err(BuildStepError::Output)
    }
}

#[derive(Debug)]
pub struct StepReport<T> {
    pub(crate) value: T,
    pub(crate) duration: Duration,
    pub(crate) log: Option<String>,
}

impl<T> StepReport<T> {
    pub fn new(value: T, duration: Duration, log: Option<String>) -> Self {
        Self {
            value,
            duration,
            log,
        }
    }

    pub fn into_inner(self) -> T {
        self.value
    }

    pub fn log(&self) -> Option<&str> {
        self.log.as_deref().filter(|log| !log.trim().is_empty())
    }

    pub fn value(&self) -> &T {
        &self.value
    }
}

pub type StepFailure = StepReport<BuildStepError>;

impl fmt::Display for StepFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "build step failed after {:?}: {}",
            self.duration, self.value,
        )
    }
}

impl std::error::Error for StepFailure {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.value)
    }
}

pub type StepResult<T> = Result<StepReport<T>, StepFailure>;

pub trait StepResultExt<T> {
    fn duration(&self) -> Duration;
    fn log(&self) -> Option<&str>;
    fn into_inner(self) -> Result<T, BuildStepError>;
    fn as_inner(&self) -> Result<&T, &BuildStepError>;
}

impl<T> StepResultExt<T> for StepResult<T> {
    fn duration(&self) -> Duration {
        match self {
            Ok(report) => report.duration,
            Err(report) => report.duration,
        }
    }

    fn log(&self) -> Option<&str> {
        match self {
            Ok(report) => report.log(),
            Err(report) => report.log(),
        }
    }

    fn into_inner(self) -> Result<T, BuildStepError> {
        match self {
            Ok(report) => Ok(report.value),
            Err(report) => Err(report.value),
        }
    }

    fn as_inner(&self) -> Result<&T, &BuildStepError> {
        self.as_ref()
            .map(|report| &report.value)
            .map_err(|report| &report.value)
    }
}
