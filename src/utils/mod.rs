//! Various utilities for docs.rs

pub(crate) use self::cargo_metadata::{CargoMetadata, Package as MetadataPackage};
pub(crate) use self::copy::copy_dir_all;
pub use self::daemon::{start_daemon, watch_registry};
pub(crate) use self::html::rewrite_rustdoc_html_stream;
pub use self::queue::{
    get_crate_pattern_and_priority, get_crate_priority, list_crate_priorities,
    remove_crate_priority, set_crate_priority,
};
pub use self::queue_builder::queue_builder;
pub(crate) use self::rustc_version::{get_correct_docsrs_style_file, parse_rustc_version};

#[cfg(test)]
pub(crate) use self::cargo_metadata::{Dependency, Target};

mod cargo_metadata;
pub mod consistency;
mod copy;
pub mod daemon;
mod html;
mod queue;
pub(crate) mod queue_builder;
pub(crate) mod rustc_version;
use anyhow::{Context as _, Result};
use serde::Serialize;
use serde::de::DeserializeOwned;
use std::{fmt, panic};
use tracing::{Span, error, warn};
pub(crate) mod sized_buffer;

use std::{future::Future, thread, time::Duration};

pub(crate) fn report_error(err: &anyhow::Error) {
    // Debug-format for anyhow errors includes context & backtrace
    if std::env::var("SENTRY_DSN").is_ok() {
        sentry::integrations::anyhow::capture_anyhow(err);
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
    Toolchain,
}

pub async fn set_config(
    conn: &mut sqlx::PgConnection,
    name: ConfigName,
    value: impl Serialize,
) -> anyhow::Result<()> {
    let name: &'static str = name.into();
    sqlx::query!(
        "INSERT INTO config (name, value)
         VALUES ($1, $2)
         ON CONFLICT (name) DO UPDATE SET value = $2;",
        name,
        &serde_json::to_value(value)?,
    )
    .execute(conn)
    .await?;
    Ok(())
}

pub async fn get_config<T>(conn: &mut sqlx::PgConnection, name: ConfigName) -> Result<Option<T>>
where
    T: DeserializeOwned,
{
    let name: &'static str = name.into();
    Ok(
        match sqlx::query!("SELECT value FROM config WHERE name = $1;", name)
            .fetch_optional(conn)
            .await?
        {
            Some(row) => serde_json::from_value(row.value)?,
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
/// ```text,ignore
/// let data = spawn_blocking(move || -> anyhow::Result<_> {
///     let data = get_the_data()?;
///     Ok(data)
/// })
/// .await
/// .context("failed to join thread")??;
/// ```
///
/// with this helper function:
/// ```text,ignore
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
    let span = Span::current();

    let result = tokio::task::spawn_blocking(move || {
        let _guard = span.enter();
        f()
    })
    .await;

    match result {
        Ok(result) => result,
        Err(err) if err.is_panic() => panic::resume_unwind(err.into_panic()),
        Err(err) => Err(err.into()),
    }
}

/// Move the execution of a blocking function into a separate, new thread.
///
/// Only for long-running / expensive operations that would block the async runtime or its
/// blocking workerpool.
///
/// The rule should be:
/// * async stuff -> in the tokio runtime, other async functions
/// * blocking I/O -> `spawn_blocking`
/// * CPU-Bound things:
///   - `render_in_threadpool` (continious load like rendering)
///   - `run_blocking` (sporadic CPU bound load)
///
/// The thread-name will help us better seeing where our CPU load is coming from on the
/// servers.
///
/// Generally speaking, using tokio's `spawn_blocking` is also ok-ish, if the work is sporadic.
/// But then I wouldn't get thread-names.
pub(crate) async fn run_blocking<N, R, F>(name: N, f: F) -> Result<R>
where
    N: Into<String> + fmt::Display,
    F: FnOnce() -> Result<R> + Send + 'static,
    R: Send + 'static,
{
    let name = name.into();
    let span = tracing::Span::current();
    let (send, recv) = tokio::sync::oneshot::channel();
    thread::Builder::new()
        .name(format!("docsrs-{name}"))
        .spawn(move || {
            let _guard = span.enter();

            // `.send` only fails when the receiver is dropped while we work,
            // at which point we don't need the result anymore.
            let _ = send.send(f());
        })
        .with_context(|| format!("couldn't spawn worker thread for {}", &name))?;

    recv.await.context("sender was dropped")?
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

pub(crate) async fn retry_async<T, Fut, F: FnMut() -> Fut>(mut f: F, max_attempts: u32) -> Result<T>
where
    Fut: Future<Output = Result<T>>,
{
    for attempt in 1.. {
        match f().await {
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
                    tokio::time::sleep(Duration::from_secs(sleep_for as u64)).await;
                }
            }
        }
    }
    unreachable!();
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test::async_wrapper;
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
        async_wrapper(|env| async move {
            let mut conn = env.async_db().async_conn().await;
            sqlx::query!("DELETE FROM config")
                .execute(&mut *conn)
                .await?;

            assert!(
                get_config::<String>(&mut conn, ConfigName::RustcVersion)
                    .await?
                    .is_none()
            );
            Ok(())
        });
    }

    #[test]
    fn test_set_and_get_config_() {
        async_wrapper(|env| async move {
            let mut conn = env.async_db().async_conn().await;
            sqlx::query!("DELETE FROM config")
                .execute(&mut *conn)
                .await?;

            assert!(
                get_config::<String>(&mut conn, ConfigName::RustcVersion)
                    .await?
                    .is_none()
            );

            set_config(
                &mut conn,
                ConfigName::RustcVersion,
                Value::String("some value".into()),
            )
            .await?;
            assert_eq!(
                get_config(&mut conn, ConfigName::RustcVersion).await?,
                Some("some value".to_string())
            );
            Ok(())
        });
    }
}
