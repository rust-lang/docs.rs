//! The service-independent parts of a docs.rs documentation build.
//!
//! This crate contains the canonical rustwide workspace, sandbox, and Cargo
//! command configuration used to build documentation. Production concerns such
//! as the build queue, database records, and artifact storage belong in
//! `docs_rs_builder`, while local and CI frontends can use this crate directly.

mod build;
mod command;
mod release;
mod result;
mod sandbox;
mod utils;
mod workspace;

pub use build::{DOC_OUTPUT_DIR_NAME, ReleaseBuild};
pub use command::PrepareCommand;
pub use release::{FetchedRelease, ReleaseContext};
pub use result::{
    BuildStepError, ReleaseBuildResult, RustdocJsonOutput, StepResult, TargetBuildResult,
};
pub use sandbox::CpuLimit;
pub use utils::resolve_sandbox_image;
pub use workspace::{BuildEnvironment, SandboxImageSource};

/// Version of docs.rs whose build behavior this crate implements.
pub const BUILDER_VERSION: &str = docs_rs_utils::BUILD_VERSION;
