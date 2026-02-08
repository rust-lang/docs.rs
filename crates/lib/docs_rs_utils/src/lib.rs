mod runtime_ext;
pub mod rustc_version;
#[cfg(feature = "testing")]
pub mod testing;

pub use runtime_ext::Handle;

use anyhow::{Context as _, Result};
use std::fmt;
use std::{panic, thread, time::Duration};
use tokio::runtime;
use tracing::{Span, error, warn};

/// Version string generated at build time contains last git
/// commit hash and build date
pub const BUILD_VERSION: &str = concat!(
    env!("CARGO_PKG_VERSION"),
    " (",
    env!("GIT_SHA"),
    " ",
    env!("BUILD_DATE"),
    " )"
);

pub const APP_USER_AGENT: &str = concat!(
    env!("CARGO_PKG_NAME"),
    " ",
    " (",
    env!("GIT_SHA"),
    " ",
    env!("BUILD_DATE"),
    " )"
);

/// Where rustdoc's static files are stored in S3.
/// Since the prefix starts with `/`, it needs to be referenced with a double slash in
/// API & AWS CLI.
/// Example:
/// `s3://rust-docs-rs//rustdoc-static/something.css`
pub const RUSTDOC_STATIC_STORAGE_PREFIX: &str = "/rustdoc-static/";

/// Where we want to serve the rustdoc static files stored in the storage prefix above
pub const RUSTDOC_STATIC_PATH: &str = "/-/rustdoc.static/";

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
pub async fn spawn_blocking<F, R>(f: F) -> Result<R>
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

pub fn retry<T>(mut f: impl FnMut() -> Result<T>, max_attempts: u32) -> Result<T> {
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

pub async fn retry_async<T, Fut, F: FnMut() -> Fut>(mut f: F, max_attempts: u32) -> Result<T>
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

pub fn start_async_cron<F, Fut>(name: &'static str, interval: Duration, exec: F)
where
    Fut: Future<Output = Result<()>> + Send,
    F: Fn() -> Fut + Send + 'static,
{
    start_async_cron_in_runtime(&runtime::Handle::current(), name, interval, exec)
}

pub fn start_async_cron_in_runtime<F, Fut>(
    runtime: &runtime::Handle,
    name: &'static str,
    interval: Duration,
    exec: F,
) where
    Fut: Future<Output = Result<()>> + Send,
    F: Fn() -> Fut + Send + 'static,
{
    runtime.spawn(async move {
        let mut interval = tokio::time::interval(interval);
        loop {
            interval.tick().await;
            if let Err(err) = exec().await {
                // FIXME: is there value in report_error over tracing::error!?
                error!(?err, name, "failed to run scheduled task");
            }
        }
    });
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
pub async fn run_blocking<N, R, F>(name: N, f: F) -> Result<R>
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
