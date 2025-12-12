use anyhow::{Context as _, Result};
use std::fmt;
use std::thread;

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
