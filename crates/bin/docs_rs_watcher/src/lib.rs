mod config;
pub mod consistency;
mod db;
mod index;
pub mod index_watcher;
mod rebuilds;
mod service_metrics;
#[cfg(test)]
mod testing;

pub use config::Config;
pub use db::{delete_crate, delete_version};
pub use index::Index;
pub use rebuilds::queue_rebuilds;

use crate::{index_watcher::get_new_crates, service_metrics::OtelServiceMetrics};
use anyhow::Result;
use docs_rs_context::Context;
use docs_rs_utils::start_async_cron;
use std::{sync::Arc, time::Duration};
use tokio::time::{self, Instant};
use tracing::{debug, error, info, trace};

/// Run the registry watcher
/// NOTE: this should only be run once, otherwise crates would be added
/// to the queue multiple times.
pub async fn watch_registry(config: &Config, context: &Context) -> Result<()> {
    let mut last_gc = Instant::now();

    let queue = context.build_queue()?;

    loop {
        if queue.is_locked().await? {
            debug!("Queue is locked, skipping checking new crates");
        } else {
            debug!("Checking new crates");
            let index = Index::from_config(config).await?;

            match get_new_crates(context, &index).await {
                Ok(n) => debug!("{} crates added to queue", n),
                Err(e) => {
                    error!(?e, "Failed to get new crates");
                }
            }

            if last_gc.elapsed().as_secs() >= config.registry_gc_interval {
                index.run_git_gc().await;
                last_gc = Instant::now();
            }
        }
        time::sleep(config.delay_between_registry_fetches).await;
    }
}

pub async fn start_background_service_metric_collector(context: &Context) -> Result<()> {
    let build_queue = context.build_queue()?.clone();
    let service_metrics = Arc::new(OtelServiceMetrics::new(&context.meter_provider));

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

pub async fn start_background_queue_rebuild(config: Arc<Config>, context: &Context) -> Result<()> {
    let pool = context.pool()?.clone();
    let build_queue = context.build_queue()?.clone();

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

pub async fn start_background_repository_stats_updater(context: &Context) -> Result<()> {
    // This call will still skip github repositories updates and continue if no token is provided
    // (gitlab doesn't require to have a token). The only time this can return an error is when
    // creating a pool or if config fails, which shouldn't happen here because this is run right at
    // startup.
    let updater = context.repository_stats()?.clone();
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
    Ok(())
}
