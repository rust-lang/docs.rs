//! [Docs.rs](https://docs.rs) (formerly cratesfyi) is an open source project to host
//! documentation of crates for the Rust Programming Language.
#![allow(
    clippy::cognitive_complexity,
    // TODO: `AxumNope::Redirect(EscapedURI, CachePolicy)` is too big.
    clippy::result_large_err,
)]

pub use self::config::Config;
pub use self::context::Context;

pub use docs_rs_build_limits::DEFAULT_MAX_TARGETS;
pub use docs_rs_utils::{APP_USER_AGENT, BUILD_VERSION, RUSTDOC_STATIC_STORAGE_PREFIX};

mod config;
mod context;
pub mod utils;
