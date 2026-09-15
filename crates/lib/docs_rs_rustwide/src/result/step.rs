use anyhow::Result;
use docs_rs_types::BuildError;
use rustwide::cmd::CommandError;
use std::time::Duration;

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
    pub value: T,
    pub duration: Duration,
    pub log: Option<String>,
}

pub type StepFailure = StepReport<BuildStepError>;

pub type StepResult<T> = Result<StepReport<T>, StepFailure>;

pub trait StepResultExt {
    fn duration(&self) -> Duration;
    fn log(&self) -> Option<&str>;
}

impl<T> StepResultExt for StepResult<T> {
    fn duration(&self) -> Duration {
        match self {
            Ok(report) => report.duration,
            Err(report) => report.duration,
        }
    }

    fn log(&self) -> Option<&str> {
        match self {
            Ok(report) => report.log.as_deref(),
            Err(report) => report.log.as_deref(),
        }
    }
}
