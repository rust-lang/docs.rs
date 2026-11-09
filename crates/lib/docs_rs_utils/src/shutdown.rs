use anyhow::{Result, bail};
use std::sync::atomic::{AtomicBool, Ordering};
use tokio::runtime;
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};

static INSTALLED: AtomicBool = AtomicBool::new(false);

/// Install SIGINT / SIGTERM handlers and return a token that is cancelled on
/// the first signal. A second signal exits the process immediately.
///
/// This replaces the default signal behaviour for the rest of the process,
/// so only call it in long-running services that actually watch the token.
/// One-off CLI commands should not, otherwise Ctrl-C stops working for them.
///
/// Can only be called once per process.
pub fn install_shutdown_signal_handlers(runtime: &runtime::Handle) -> Result<CancellationToken> {
    if INSTALLED.swap(true, Ordering::SeqCst) {
        bail!("shutdown signal handlers are already installed");
    }

    let shutdown = CancellationToken::new();

    runtime.spawn({
        let shutdown = shutdown.clone();
        async move {
            shutdown_signal().await;
            info!(
                "shutdown requested, finishing in-flight work; send the signal again to exit immediately"
            );
            shutdown.cancel();

            shutdown_signal().await;
            warn!("second shutdown signal received, exiting immediately");
            std::process::exit(1);
        }
    });

    Ok(shutdown)
}

/// Wait for SIGINT (Ctrl-C) or, on unix, SIGTERM.
async fn shutdown_signal() {
    let ctrl_c = async {
        tokio::signal::ctrl_c()
            .await
            .expect("failed to install Ctrl+C handler");
    };

    #[cfg(unix)]
    let terminate = async {
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("failed to install signal handler")
            .recv()
            .await;
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {},
        _ = terminate => {},
    }
}
