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

/// Outcome and diagnostics of one release build step, including failures.
#[derive(Debug)]
pub struct StepResult<T> {
    /// Wall-clock time spent preparing, executing, and processing this step.
    pub duration: Duration,
    /// Produced value or the phase in which the step failed.
    pub outcome: Result<T, BuildStepError>,
    /// Cargo and rustdoc output captured for this step.
    pub log: Option<String>,
}

impl<T> StepResult<T> {
    /// Whether this step completed successfully.
    pub fn successful(&self) -> bool {
        self.outcome.is_ok()
    }

    pub fn log(&self) -> &str {
        self.log.as_deref().unwrap_or_default()
    }
}
