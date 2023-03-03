//! Various utilities for docs.rs

pub(crate) use self::cargo_metadata::{CargoMetadata, Package as MetadataPackage};
pub(crate) use self::copy::copy_dir_all;
pub use self::daemon::{start_daemon, watch_registry};
pub(crate) use self::html::rewrite_lol;
pub use self::queue::{get_crate_priority, remove_crate_priority, set_crate_priority};
pub use self::queue_builder::queue_builder;
pub(crate) use self::rustc_version::{get_correct_docsrs_style_file, parse_rustc_version};

#[cfg(test)]
pub(crate) use self::cargo_metadata::{Dependency, Target};

mod cargo_metadata;
#[cfg(feature = "consistency_check")]
pub mod consistency;
mod copy;
pub mod daemon;
mod html;
mod queue;
pub(crate) mod queue_builder;
mod rustc_version;
use anyhow::{Context as _, Result};
use postgres::Client;
use serde::de::DeserializeOwned;
use serde::Serialize;
use tracing::error;
pub(crate) mod sized_buffer;

use std::thread;
use std::time::Duration;
use tracing::warn;

pub(crate) const APP_USER_AGENT: &str = concat!(
    env!("CARGO_PKG_NAME"),
    " ",
    include_str!(concat!(env!("OUT_DIR"), "/git_version"))
);

pub(crate) fn report_error(err: &anyhow::Error) {
    // Debug-format for anyhow errors includes context & backtrace
    if std::env::var("SENTRY_DSN").is_ok() {
        sentry_anyhow::capture_anyhow(err);
        error!(reported_to_sentry = true, "{err:?}");
    } else {
        error!("{err:?}");
    }
}

#[derive(strum::IntoStaticStr)]
#[strum(serialize_all = "snake_case")]
pub enum ConfigName {
    RustcVersion,
    LastSeenIndexReference,
    QueueLocked,
}

pub fn set_config(
    conn: &mut Client,
    name: ConfigName,
    value: impl Serialize,
) -> anyhow::Result<()> {
    let name: &'static str = name.into();
    conn.execute(
        "INSERT INTO config (name, value) 
        VALUES ($1, $2)
        ON CONFLICT (name) DO UPDATE SET value = $2;",
        &[&name, &serde_json::to_value(value)?],
    )?;
    Ok(())
}

pub fn get_config<T>(conn: &mut Client, name: ConfigName) -> Result<Option<T>>
where
    T: DeserializeOwned,
{
    let name: &'static str = name.into();
    Ok(
        match conn.query_opt("SELECT value FROM config WHERE name = $1;", &[&name])? {
            Some(row) => serde_json::from_value(row.get("value"))?,
            None => None,
        },
    )
}

/// a wrapper around tokio's `spawn_blocking` that
/// enables us to write nicer code when the closure
/// returns an `anyhow::Result`.
///
/// The join-error will also be converted into an `anyhow::Error`.
///
/// with standard `tokio::task::spawn_blocking`:
/// ```ignore
/// let data = spawn_blocking(move || -> anyhow::Result<_> {
///     let data = get_the_data()?;
///     Ok(data)
/// })
/// .await
/// .context("failed to join thread")??;
/// ```
///
/// with this helper function:
/// ```ignore
/// let data = spawn_blocking(move || {
///     let data = get_the_data()?;
///     Ok(data)
/// })
/// .await?
/// ```
pub(crate) async fn spawn_blocking<F, R>(f: F) -> Result<R>
where
    F: FnOnce() -> Result<R> + Send + 'static,
    R: Send + 'static,
{
    tokio::task::spawn_blocking(f)
        .await
        .context("failed to join thread")?
}

pub(crate) fn retry<T>(mut f: impl FnMut() -> Result<T>, max_attempts: u32) -> Result<T> {
    for attempt in 1.. {
        match f() {
            Ok(result) => return Ok(result),
            Err(err) => {
                if attempt > max_attempts {
                    return Err(err);
                } else {
                    let sleep_for = 2u32.pow(attempt);
                    warn!(
                        "got error on attempt {}, will try again after {}s:\n{:?}",
                        attempt, sleep_for, err
                    );
                    thread::sleep(Duration::from_secs(sleep_for as u64));
                }
            }
        }
    }
    unreachable!()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test::wrapper;
    use serde_json::Value;
    use test_case::test_case;

    #[test_case(ConfigName::RustcVersion, "rustc_version")]
    #[test_case(ConfigName::QueueLocked, "queue_locked")]
    #[test_case(ConfigName::LastSeenIndexReference, "last_seen_index_reference")]
    fn test_configname_variants(variant: ConfigName, expected: &'static str) {
        let name: &'static str = variant.into();
        assert_eq!(name, expected);
    }

    #[test]
    fn test_get_config_empty() {
        wrapper(|env| {
            let mut conn = env.db().conn();
            conn.execute("DELETE FROM config", &[])?;

            assert!(get_config::<String>(&mut conn, ConfigName::RustcVersion)?.is_none());
            Ok(())
        });
    }

    #[test]
    fn test_set_and_get_config_() {
        wrapper(|env| {
            let mut conn = env.db().conn();
            conn.execute("DELETE FROM config", &[])?;

            assert!(get_config::<String>(&mut conn, ConfigName::RustcVersion)?.is_none());

            set_config(
                &mut conn,
                ConfigName::RustcVersion,
                Value::String("some value".into()),
            )?;
            assert_eq!(
                get_config(&mut conn, ConfigName::RustcVersion)?,
                Some("some value".to_string())
            );
            Ok(())
        });
    }
}
