#![allow(
    // clippy::cognitive_complexity,
    // TODO: `AxumNope::Redirect(EscapedURI, CachePolicy)` is too big.
    clippy::result_large_err,
)]

mod cache;
mod config;
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
pub use docs_rs_build_limits::DEFAULT_MAX_TARGETS;
pub use docs_rs_utils::{APP_USER_AGENT, BUILD_VERSION, RUSTDOC_STATIC_STORAGE_PREFIX};
pub use font_awesome_as_a_crate::icons;
pub use handlers::run_web_server;

use page::GlobalAlert;

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
