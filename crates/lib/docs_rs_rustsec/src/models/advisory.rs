//! RustSec-specific metadata embedded in OSV advisories.
mod date;
mod id;
mod informational;

pub use id::Id;
pub use informational::Informational;
