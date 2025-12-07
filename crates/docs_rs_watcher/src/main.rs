use anyhow::Context as _;
use clap::Parser;
use docs_rs_build_queue::AsyncBuildQueue;
use docs_rs_watcher::{Config, watch_registry};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let _guard = docs_rs_logging::init().context("error initializing logging")?;

    let args = Cli::try_parse()?;

    let config = Config::from_environment()?;

    let build_queue = AsyncBuildQueue::new();

    if args.repository_stats_updater {
        // start_background_repository_stats_updater(&ctx)?;
    }
    if args.queue_rebuilds {
        // start_background_queue_rebuild(&ctx)?;
    }

    // When people run the services separately, we assume that we can collect service
    // metrics from the registry watcher, which should only run once, and all the time.
    start_background_service_metric_collector(&ctx)?;

    watch_registry(&async_build_queue, &config).await?;

    Ok(())
}

#[derive(Parser)]
#[command(
    about = env!("CARGO_PKG_DESCRIPTION"),
    version = docs_rs_utils::BUILD_VERSION,
    rename_all = "kebab-case",
)]
struct Cli {
    /// Enable or disable the repository stats updater
    #[arg(long = "repository-stats-updater")]
    repository_stats_updater: bool,

    /// Enable or disable rebuild queueing
    #[arg(long = "queue-rebuilds")]
    queue_rebuilds: bool,
}
