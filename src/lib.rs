//! [Docs.rs](https://docs.rs) (formerly cratesfyi) is an open source project to host
//! documentation of crates for the Rust Programming Language.
#![allow(
    clippy::cognitive_complexity,
    // TODO: `AxumNope::Redirect(EscapedURI, CachePolicy)` is too big.
    clippy::result_large_err,
)]

pub use self::config::Config;
pub use self::context::Context;
pub use self::web::start_web_server;

pub use docs_rs_build_limits::DEFAULT_MAX_TARGETS;
pub use docs_rs_utils::{APP_USER_AGENT, BUILD_VERSION, RUSTDOC_STATIC_STORAGE_PREFIX};
pub use font_awesome_as_a_crate::icons;

mod config;
mod context;
mod error;
pub mod metrics;
#[cfg(test)]
mod test;
pub mod utils;
mod web;

use web::page::GlobalAlert;

// Warning message shown in the navigation bar of every page. Set to `None` to hide it.
pub(crate) static GLOBAL_ALERT: Option<GlobalAlert> = None;
/*
pub(crate) static GLOBAL_ALERT: Option<GlobalAlert> = Some(GlobalAlert {
    url: "https://blog.rust-lang.org/2019/09/18/upcoming-docsrs-changes.html",
    text: "Upcoming docs.rs breaking changes!",
    css_class: "error",
    fa_icon: "exclamation-triangle",
});
*/
