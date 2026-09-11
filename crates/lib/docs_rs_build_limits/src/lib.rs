#[cfg(feature = "database")]
pub mod blacklist;
mod config;
mod limits;
#[cfg(feature = "database")]
mod overrides;

pub use config::Config;
pub use limits::Limits;
#[cfg(feature = "database")]
pub use overrides::Overrides;

/// Maximum number of targets allowed for a crate to be documented on.
pub const DEFAULT_MAX_TARGETS: usize = 10;
