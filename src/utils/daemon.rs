//! Simple daemon
//!
//! This daemon will start web server, track new packages and build them

use crate::{
    Context, RustwideBuilder,
    metrics::service::OtelServiceMetrics,
    utils::{queue_builder, report_error},
    web::start_web_server,
};
use anyhow::{Context as _, Error, anyhow};
use docs_rs_build_queue::rebuilds::queue_rebuilds;
use std::future::Future;
use std::sync::Arc;
use std::thread;
use std::time::Duration;
use tokio::runtime;
use tracing::{info, trace};

pub fn start_background_service_metric_collector(context: &Context) -> Result<(), Error> {
    let runtime = context.runtime.clone();
    let build_queue = context.async_build_queue.clone();
    let service_metrics = Arc::new(OtelServiceMetrics::new(&context.meter_provider));

    async_cron(
        &runtime,
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

fn start_registry_watcher(context: &Context) -> Result<(), Error> {
    let build_queue = context.async_build_queue.clone();
    let config = context.config.clone();

    context.runtime.spawn(async move {
        // space this out to prevent it from clashing against the queue-builder thread on launch
        tokio::time::sleep(Duration::from_secs(30)).await;

        docs_rs_watcher::watch_registry(&build_queue, &config.watcher).await
    });

    Ok(())
}

pub fn start_background_queue_rebuild(context: &Context) -> Result<(), Error> {
    let runtime = context.runtime.clone();
    let pool = context.pool.clone();
    let config = context.config.clone();
    let build_queue = context.async_build_queue.clone();

    if config.build_queue.max_queued_rebuilds.is_none() {
        info!("rebuild config incomplete, skipping rebuild queueing");
        return Ok(());
    }

    async_cron(
        &runtime,
        "background queue rebuilder",
        Duration::from_secs(60 * 60),
        move || {
            let pool = pool.clone();
            let build_queue = build_queue.clone();
            let config = config.clone();
            async move {
                let mut conn = pool.get_async().await?;
                queue_rebuilds(&mut conn, &config.build_queue, &build_queue).await?;
                Ok(())
            }
        },
    );
    Ok(())
}

pub fn start_background_repository_stats_updater(context: &Context) -> Result<(), Error> {
    // This call will still skip github repositories updates and continue if no token is provided
    // (gitlab doesn't require to have a token). The only time this can return an error is when
    // creating a pool or if config fails, which shouldn't happen here because this is run right at
    // startup.
    let updater = context.repository_stats_updater.clone();
    let runtime = context.runtime.clone();
    async_cron(
        &runtime,
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

pub fn start_daemon(context: Context, enable_registry_watcher: bool) -> Result<(), Error> {
    let context = Arc::new(context);

    // Start the web server before doing anything more expensive
    // Please check with an administrator before changing this (see #1172 for context).
    info!("Starting web server");
    let webserver_thread = thread::spawn({
        let context = context.clone();
        move || start_web_server(None, &context)
    });

    if enable_registry_watcher {
        // check new crates every minute
        start_registry_watcher(&context)?;
    }

    // build new crates every minute
    let rustwide_builder = RustwideBuilder::init(&context)?;
    thread::Builder::new()
        .name("build queue reader".to_string())
        .spawn({
            let context = context.clone();
            move || queue_builder(&context, rustwide_builder).unwrap()
        })
        .unwrap();

    start_background_repository_stats_updater(&context)?;
    start_background_queue_rebuild(&context)?;

    // when people run the daemon, we assume the daemon is the one single process where
    // we can collect the service metrics.
    start_background_service_metric_collector(&context)?;

    // NOTE: if a error occurred earlier in `start_daemon`, the server will _not_ be joined -
    // instead it will get killed when the process exits.
    webserver_thread
        .join()
        .map_err(|err| anyhow!("web server panicked: {:?}", err))?
}

pub(crate) fn async_cron<F, Fut>(
    runtime: &runtime::Handle,
    name: &'static str,
    interval: Duration,
    exec: F,
) where
    Fut: Future<Output = Result<(), Error>> + Send,
    F: Fn() -> Fut + Send + 'static,
{
    runtime.spawn(async move {
        let mut interval = tokio::time::interval(interval);
        loop {
            interval.tick().await;
            if let Err(err) = exec()
                .await
                .with_context(|| format!("failed to run scheduled task '{name}'"))
            {
                report_error(&err);
            }
        }
    });
}
