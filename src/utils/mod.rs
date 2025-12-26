//! Various utilities for docs.rs

pub use self::daemon::{start_daemon, watch_registry};
pub(crate) use self::html::rewrite_rustdoc_html_stream;

pub mod consistency;
pub mod daemon;
mod html;

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
