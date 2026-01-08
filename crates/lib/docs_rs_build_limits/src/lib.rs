pub mod blacklist;
mod config;
mod limits;
mod overrides;

pub use config::Config;
pub use limits::Limits;
pub use overrides::Overrides;

/// Maximum number of targets allowed for a crate to be documented on.
pub const DEFAULT_MAX_TARGETS: usize = 10;
