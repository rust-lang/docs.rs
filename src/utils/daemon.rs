//! Simple daemon
//!
//! This daemon will start web server, track new packages and build them

use crate::{
    cdn, queue_rebuilds,
    utils::{queue_builder, report_error},
    web::start_web_server,
    AsyncBuildQueue, Config, Context, Index, RustwideBuilder,
};
use anyhow::{anyhow, Context as _, Error};
use std::future::Future;
use std::sync::Arc;
use std::thread;
use std::time::Duration;
use tokio::{runtime::Runtime, task::spawn_blocking, time::Instant};
use tracing::{debug, info};

/// Run the registry watcher
/// NOTE: this should only be run once, otherwise crates would be added
/// to the queue multiple times.
pub async fn watch_registry(
    build_queue: Arc<AsyncBuildQueue>,
    config: Arc<Config>,
    index: Arc<Index>,
) -> Result<(), Error> {
    let mut last_gc = Instant::now();

    loop {
        if build_queue.is_locked().await? {
            debug!("Queue is locked, skipping checking new crates");
        } else {
            debug!("Checking new crates");
            match build_queue
                .get_new_crates(&index)
                .await
                .context("Failed to get new crates")
            {
                Ok(n) => debug!("{} crates added to queue", n),
                Err(e) => report_error(&e),
            }
        }

        if last_gc.elapsed().as_secs() >= config.registry_gc_interval {
            spawn_blocking({
                let index = index.clone();
                move || index.run_git_gc()
            })
            .await?;
            last_gc = Instant::now();
        }
        tokio::time::sleep(Duration::from_secs(60)).await;
    }
}

fn start_registry_watcher(context: &dyn Context) -> Result<(), Error> {
    let build_queue = context.runtime()?.block_on(context.async_build_queue())?;
    let config = context.config()?;
    let index = context.index()?;
    let runtime = context.runtime()?;

    runtime.spawn(async {
        // space this out to prevent it from clashing against the queue-builder thread on launch
        tokio::time::sleep(Duration::from_secs(30)).await;

        watch_registry(build_queue, config, index).await
    });

    Ok(())
}

pub fn start_background_repository_stats_updater(context: &dyn Context) -> Result<(), Error> {
    // This call will still skip github repositories updates and continue if no token is provided
    // (gitlab doesn't require to have a token). The only time this can return an error is when
    // creating a pool or if config fails, which shouldn't happen here because this is run right at
    // startup.
    let updater = context.repository_stats_updater()?;
    let runtime = context.runtime()?;
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

pub fn start_background_queue_rebuild(context: &dyn Context) -> Result<(), Error> {
    let runtime = context.runtime()?;
    let pool = context.pool()?;
    let config = context.config()?;
    let build_queue = runtime.block_on(context.async_build_queue())?;

    if config.max_queued_rebuilds.is_none() || config.rebuild_up_to_date.is_none() {
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
                queue_rebuilds(&mut conn, &config, &build_queue).await?;
                Ok(())
            }
        },
    );
    Ok(())
}

pub fn start_background_cdn_invalidator(context: &dyn Context) -> Result<(), Error> {
    let metrics = context.instance_metrics()?;
    let config = context.config()?;
    let pool = context.pool()?;
    let runtime = context.runtime()?;
    let cdn = runtime.block_on(context.cdn())?;

    if config.cloudfront_distribution_id_web.is_none()
        && config.cloudfront_distribution_id_static.is_none()
    {
        info!("no cloudfront distribution IDs found, skipping background cdn invalidation");
        return Ok(());
    }

    if !config.cache_invalidatable_responses {
        info!("full page cache disabled, skipping background cdn invalidation");
        return Ok(());
    }

    async_cron(
        &runtime,
        "cdn invalidator",
        Duration::from_secs(60),
        move || {
            let pool = pool.clone();
            let config = config.clone();
            let cdn = cdn.clone();
            let metrics = metrics.clone();
            async move {
                let mut conn = pool.get_async().await?;
                if let Some(distribution_id) = config.cloudfront_distribution_id_web.as_ref() {
                    cdn::handle_queued_invalidation_requests(
                        &config,
                        &cdn,
                        &metrics,
                        &mut conn,
                        distribution_id,
                    )
                    .await
                    .context("error handling queued invalidations for web CDN invalidation")?;
                }
                if let Some(distribution_id) = config.cloudfront_distribution_id_static.as_ref() {
                    cdn::handle_queued_invalidation_requests(
                        &config,
                        &cdn,
                        &metrics,
                        &mut conn,
                        distribution_id,
                    )
                    .await
                    .context("error handling queued invalidations for static CDN invalidation")?;
                }
                Ok(())
            }
        },
    );
    Ok(())
}

pub fn start_daemon<C: Context + Send + Sync + 'static>(
    context: C,
    enable_registry_watcher: bool,
) -> Result<(), Error> {
    let context = Arc::new(context);

    // Start the web server before doing anything more expensive
    // Please check with an administrator before changing this (see #1172 for context).
    info!("Starting web server");
    let webserver_thread = thread::spawn({
        let context = context.clone();
        move || start_web_server(None, &*context)
    });

    if enable_registry_watcher {
        // check new crates every minute
        start_registry_watcher(&*context)?;
    }

    // build new crates every minute
    let build_queue = context.build_queue()?;
    let config = context.config()?;
    let rustwide_builder = RustwideBuilder::init(&*context)?;
    thread::Builder::new()
        .name("build queue reader".to_string())
        .spawn({
            let context = context.clone();
            move || queue_builder(&*context, rustwide_builder, build_queue, config).unwrap()
        })
        .unwrap();

    start_background_repository_stats_updater(&*context)?;
    start_background_cdn_invalidator(&*context)?;
    start_background_queue_rebuild(&*context)?;

    // NOTE: if a error occurred earlier in `start_daemon`, the server will _not_ be joined -
    // instead it will get killed when the process exits.
    webserver_thread
        .join()
        .map_err(|err| anyhow!("web server panicked: {:?}", err))?
}

pub(crate) fn async_cron<F, Fut>(runtime: &Runtime, name: &'static str, interval: Duration, exec: F)
where
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
