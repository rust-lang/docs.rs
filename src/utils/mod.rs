//! Various utilities for docs.rs

pub(crate) use self::{
    copy::copy_dir_all,
    html::rewrite_rustdoc_html_stream,
    rustc_version::{get_correct_docsrs_style_file, parse_rustc_version},
};
pub use self::{
    daemon::start_daemon,
    queue::{
        get_crate_pattern_and_priority, get_crate_priority, list_crate_priorities,
        remove_crate_priority, set_crate_priority,
    },
    queue_builder::queue_builder,
};

mod copy;
pub mod daemon;
mod html;
mod queue;
pub(crate) mod queue_builder;
pub(crate) mod rustc_version;

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

// #[cfg(test)]
// mod tests {
//     use super::*;
//     use crate::test::async_wrapper;
//     use serde_json::Value;
//     use test_case::test_case;

//     #[test_case(ConfigName::RustcVersion, "rustc_version")]
//     #[test_case(ConfigName::QueueLocked, "queue_locked")]
//     #[test_case(ConfigName::LastSeenIndexReference, "last_seen_index_reference")]
//     fn test_configname_variants(variant: ConfigName, expected: &'static str) {
//         let name: &'static str = variant.into();
//         assert_eq!(name, expected);
//     }

//     #[test]
//     fn test_get_config_empty() {
//         async_wrapper(|env| async move {
//             let mut conn = env.async_db().async_conn().await;
//             sqlx::query!("DELETE FROM config")
//                 .execute(&mut *conn)
//                 .await?;

//             assert!(
//                 get_config::<String>(&mut conn, ConfigName::RustcVersion)
//                     .await?
//                     .is_none()
//             );
//             Ok(())
//         });
//     }

//     #[test]
//     fn test_set_and_get_config_() {
//         async_wrapper(|env| async move {
//             let mut conn = env.async_db().async_conn().await;
//             sqlx::query!("DELETE FROM config")
//                 .execute(&mut *conn)
//                 .await?;

//             assert!(
//                 get_config::<String>(&mut conn, ConfigName::RustcVersion)
//                     .await?
//                     .is_none()
//             );

//             set_config(
//                 &mut conn,
//                 ConfigName::RustcVersion,
//                 Value::String("some value".into()),
//             )
//             .await?;
//             assert_eq!(
//                 get_config(&mut conn, ConfigName::RustcVersion).await?,
//                 Some("some value".to_string())
//             );
//             Ok(())
//         });
//     }
// }
