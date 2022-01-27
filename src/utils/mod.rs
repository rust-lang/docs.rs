//! Various utilities for docs.rs

pub(crate) use self::cargo_metadata::CargoMetadata;
pub(crate) use self::copy::copy_dir_all;
pub use self::daemon::start_daemon;
pub(crate) use self::html::rewrite_lol;
pub use self::queue::{get_crate_priority, remove_crate_priority, set_crate_priority};
pub use self::queue_builder::queue_builder;
pub(crate) use self::rustc_version::{get_correct_docsrs_style_file, parse_rustc_version};
pub use cargo_metadata::Package as MetadataPackage;

#[cfg(test)]
pub(crate) use self::cargo_metadata::{Dependency, Target};

mod cargo_metadata;
#[cfg(feature = "consistency_check")]
pub mod consistency;
mod copy;
pub(crate) mod daemon;
mod html;
mod queue;
pub(crate) mod queue_builder;
mod rustc_version;
pub(crate) mod sized_buffer;

pub(crate) const APP_USER_AGENT: &str = concat!(
    env!("CARGO_PKG_NAME"),
    " ",
    include_str!(concat!(env!("OUT_DIR"), "/git_version"))
);

pub(crate) fn report_error(err: &anyhow::Error) {
    if std::env::var("SENTRY_DSN").is_ok() {
        sentry_anyhow::capture_anyhow(err);
    } else {
        // Debug-format for anyhow errors includes context & backtrace
        log::error!("{:?}", err);
    }
}
