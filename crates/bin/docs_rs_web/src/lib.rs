#![recursion_limit = "256"]
#![allow(
    // clippy::cognitive_complexity,
    // TODO: `AxumNope::Redirect(EscapedURI, CachePolicy)` is too big.
    clippy::result_large_err,
)]

mod cache;
mod config;
mod context;
mod error;
mod extractors;
mod file;
mod handlers;
pub(crate) mod match_release;
mod metadata;
mod metrics;
pub(crate) mod middleware;
mod page;
mod routes;
#[cfg(test)]
pub(crate) mod testing;
mod utils;

pub use config::Config;
pub use context::build_context;
pub use docs_rs_build_limits::DEFAULT_MAX_TARGETS;
pub use docs_rs_utils::{APP_USER_AGENT, BUILD_VERSION, RUSTDOC_STATIC_STORAGE_PREFIX};
pub use font_awesome_as_a_crate::icons;
pub use handlers::run_web_server;
