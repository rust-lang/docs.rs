//! Various utilities for docs.rs

pub(crate) use self::{copy::copy_dir_all, html::rewrite_rustdoc_html_stream};
pub use self::{
    daemon::{start_daemon, watch_registry},
    queue::{
        get_crate_pattern_and_priority, get_crate_priority, list_crate_priorities,
        remove_crate_priority, set_crate_priority,
    },
    queue_builder::queue_builder,
};

pub mod consistency;
mod copy;
pub mod daemon;
mod html;
mod queue;
pub(crate) mod queue_builder;

use tracing::error;

pub(crate) fn report_error(err: &anyhow::Error) {
    // Debug-format for anyhow errors includes context & backtrace
    if std::env::var("SENTRY_DSN").is_ok() {
        sentry::integrations::anyhow::capture_anyhow(err);
        error!(reported_to_sentry = true, "{err:?}");
    } else {
        error!("{err:?}");
    }
}
