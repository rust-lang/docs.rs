use anyhow::Result;
use clap::Parser;
use cratesfyi::daemon::start_daemon;
use docs_rs_context::Context;
use std::env;
use tokio::runtime;

fn main() {
    // set the global log::logger for backwards compatibility
    // through rustwide.
    docs_rs_builder::logging::init();
    let guard = docs_rs_logging::init().expect("error initializing logging");

    if let Err(err) = CommandLine::parse().handle_args() {
        eprintln!("error running watcher: {:?}", err);
        drop(guard);
        std::process::exit(1);
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Parser)]
#[command(
    about = env!("CARGO_PKG_DESCRIPTION"),
    version = docs_rs_utils::BUILD_VERSION,
    rename_all = "kebab-case",
)]
enum CommandLine {
    /// Starts the daemon
    Daemon,
}

impl CommandLine {
    fn handle_args(self) -> Result<()> {
        let runtime = runtime::Builder::new_multi_thread().enable_all().build()?;
        let ctx = runtime.block_on(async {
            Context::builder()
                .with_runtime()
                .await?
                .with_meter_provider()?
                .with_pool()
                .await?
                .with_storage()
                .await?
                .with_maybe_cdn()?
                .with_build_queue()?
                .with_registry_api()?
                .with_repository_stats()?
                .with_build_limits()?
                .build()
        })?;

        match self {
            Self::Daemon => start_daemon(ctx)?,
        }

        Ok(())
    }
}
