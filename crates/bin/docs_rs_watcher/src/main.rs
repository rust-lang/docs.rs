use anyhow::{Context as _, Result};
use clap::Parser;
use docs_rs_build_queue::AsyncBuildQueue;
use docs_rs_database::Pool;
use docs_rs_opentelemetry::AnyMeterProvider;
use docs_rs_repository_stats::RepositoryStatsUpdater;
use docs_rs_utils::start_async_cron;
use docs_rs_watcher::{
    Config, rebuilds::queue_rebuilds, service_metrics::OtelServiceMetrics, watch_registry,
};
use std::{sync::Arc, time::Duration};
use tracing::{info, trace};

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

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let _guard = docs_rs_logging::init().context("error initializing logging")?;

    let args = Cli::try_parse()?;

    let context = docs_rs_context::Context::new()?
        .with_pool()
        .await?
        .with_storage()
        .await?
        .with_cdn()
        .await?
        .with_build_queue()
        .await?;

    let config = Arc::new(Config::from_environment()?);

    let pool = context.pool()?;
    let build_queue = context.build_queue()?;

    if args.repository_stats_updater {
        start_background_repository_stats_updater(&config, pool.clone());
    }
    if args.queue_rebuilds {
        start_background_queue_rebuild(config.clone(), pool.clone(), build_queue.clone())?;
    }

    // When people run the services separately, we assume that we can collect service
    // metrics from the registry watcher, which should only run once, and all the time.
    start_background_service_metric_collector(build_queue.clone(), context.meter_provider())?;

    watch_registry(&build_queue, &config, &context).await?;

    Ok(())
}

fn start_background_repository_stats_updater(config: &Config, pool: Pool) {
    // This call will still skip github repositories updates and continue if no token is provided
    // (gitlab doesn't require to have a token). The only time this can return an error is when
    // creating a pool or if config fails, which shouldn't happen here because this is run right at
    // startup.

    let updater = Arc::new(RepositoryStatsUpdater::new(&config.repository, pool));

    start_async_cron(
        "repository stats updater",
        Duration::from_secs(60 * 60),
        move || {
            let updater = updater.clone();
            async move {
                updater.update_all_crates().await?;
                Ok(())
            }
        },
    );
}

fn start_background_queue_rebuild(
    config: Arc<Config>,
    pool: Pool,
    build_queue: Arc<AsyncBuildQueue>,
) -> Result<()> {
    if config.max_queued_rebuilds.is_none() {
        info!("rebuild config incomplete, skipping rebuild queueing");
        return Ok(());
    }

    start_async_cron(
        "background queue rebuilder",
        Duration::from_secs(60 * 60),
        move || {
            let pool = pool.clone();
            let build_queue = build_queue.clone();
            let config = config.clone();
            async move {
                let mut conn = pool.get_async().await?;
                queue_rebuilds(&mut conn, &config, &build_queue).await?;
                Ok(())
            }
        },
    );
    Ok(())
}

pub fn start_background_service_metric_collector(
    build_queue: Arc<AsyncBuildQueue>,
    meter_provider: &AnyMeterProvider,
) -> Result<()> {
    let service_metrics = Arc::new(OtelServiceMetrics::new(meter_provider));

    start_async_cron(
        "background service metric collector",
        // old prometheus scrape interval seems to have been ~5s, but IMO that's far too frequent
        // for these service metrics.
        Duration::from_secs(30),
        move || {
            let build_queue = build_queue.clone();
            let service_metrics = service_metrics.clone();
            async move {
                trace!("collecting service metrics");
                service_metrics.gather(&build_queue).await
            }
        },
    );
    Ok(())
}
