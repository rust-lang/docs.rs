mod config;
pub mod consistency;
mod db;
mod index;
pub mod index_watcher;
mod metrics;
mod rebuilds;
mod service_metrics;
mod statistics;
#[cfg(test)]
mod testing;

pub use config::Config;
pub use db::{delete_crate, delete_version};
pub use index::Index;
pub use rebuilds::queue_rebuilds;

use crate::{
    index_watcher::get_new_crates,
    metrics::{EventSource, WatcherMetrics},
    service_metrics::OtelServiceMetrics,
};
use anyhow::Result;
use docs_rs_context::Context;
use docs_rs_utils::start_async_cron;
use std::{sync::Arc, time::Duration};
use tokio::time::{self, Instant};
use tracing::{debug, error, info, trace};

/// Run the registry watcher
/// Registry polling and consistency checks share a database lock.
pub async fn watch_registry(config: &Config, context: &Context) -> Result<()> {
    let mut last_gc = Instant::now();

    let queue = context.build_queue()?;
    let metrics = WatcherMetrics::new(context.meter_provider());

    let mut fetch_from_registry = async || -> anyhow::Result<()> {
        if queue.is_locked().await? {
            debug!("Queue is locked, skipping checking new crates");
        } else {
            debug!("Checking new crates");
            let mut conn = context.pool()?.get_async().await?;
            let mut index = Index::from_config(config, &mut conn).await?;

            let (added, new_reference) =
                get_new_crates(context, &mut index, config, &metrics).await?;
            debug!("{} crates added to queue", added);

            if last_gc.elapsed() >= *config.registry_gc_interval {
                debug!("running `git gc`");

                index.run_git_gc().await;
                last_gc = Instant::now();
            }
            index.set_last_seen_reference(new_reference).await?;
            index.commit().await?;
        }

        Ok(())
    };

    loop {
        if let Err(err) = fetch_from_registry().await {
            metrics.record_poll_error(EventSource::Git);
            error!(?err, "Failed to get new crates");
        }
        time::sleep(*config.delay_between_registry_fetches).await;
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

pub async fn start_background_statistics_collector(context: &Context) -> Result<()> {
    let pool = context.pool()?.clone();

    start_async_cron("statistics collector", Duration::from_mins(5), move || {
        let pool = pool.clone();
        async move {
            let mut conn = pool.get_async().await?;
            statistics::gather_documentation_statistics(&mut conn).await
        }
    });

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
        Duration::from_hours(1),
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
        Duration::from_hours(1),
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
