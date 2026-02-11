use docs_rs_types::BuildError;
use rustwide::cmd::CommandError;

#[derive(thiserror::Error, Debug)]
pub(crate) enum RustwideBuildError {
    #[error(transparent)]
    CommandError(#[from] CommandError),

    #[error(transparent)]
    Other(anyhow::Error),
}

impl From<anyhow::Error> for RustwideBuildError {
    fn from(value: anyhow::Error) -> Self {
        match value.downcast::<CommandError>() {
            Ok(err) => RustwideBuildError::CommandError(err),
            Err(err) => RustwideBuildError::Other(err),
        }
    }
}

impl BuildError for RustwideBuildError {
    fn kind(&self) -> &'static str {
        match self {
            RustwideBuildError::CommandError(err) => match err {
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
            RustwideBuildError::Other(_) => "Other",
        }
    }
}
