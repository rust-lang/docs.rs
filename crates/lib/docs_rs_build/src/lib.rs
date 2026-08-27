//! The service-independent parts of a docs.rs documentation build.
//!
//! This crate contains the canonical rustwide workspace, sandbox, and Cargo
//! command configuration used to build documentation. Production concerns such
//! as the build queue, database records, and artifact storage belong in
//! `docs_rs_builder`, while local and CI frontends can use this crate directly.

mod command;
mod output;
mod sandbox;
mod workspace;

pub use command::{BuildContext, CommandOptions};
pub use output::{DOC_OUTPUT_DIR_NAME, doc_output_dir};
pub use sandbox::{CpuLimit, sandbox_builder};
pub use workspace::{DOCS_RS_USER_AGENT, WorkspaceConfig};
