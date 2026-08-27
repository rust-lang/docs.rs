//! The service-independent parts of a docs.rs documentation build.
//!
//! This crate contains the canonical rustwide workspace, sandbox, and Cargo
//! command configuration used to build documentation. Production concerns such
//! as the build queue, database records, and artifact storage belong in
//! `docs_rs_builder`, while local and CI frontends can use this crate directly.

mod command;
mod result;
mod sandbox;
mod workspace;

pub use command::{CommandOptions, DOC_OUTPUT_DIR_NAME, ReleaseContext};
pub use result::{BuildStepError, ReleaseBuildResult, StepResult, TargetBuildResult};
pub use sandbox::{BuildEnvironment, CpuLimit};
pub use workspace::{DOCS_RS_USER_AGENT, WorkspaceConfig};
