use anyhow::{Error, anyhow};
use docs_rs_builder::{RustwideBuilder, queue_builder};
use docs_rs_config::AppConfig as _;
use docs_rs_context::Context;
use docs_rs_watcher::{
    start_background_queue_rebuild, start_background_repository_stats_updater,
    start_background_service_metric_collector, watch_registry,
};
use docs_rs_web::run_web_server;
use std::sync::Arc;
use std::thread;
use std::time::Duration;
use tracing::info;

fn start_registry_watcher(
    config: Arc<docs_rs_watcher::Config>,
    context: Arc<Context>,
) -> Result<(), Error> {
    let runtime = context.runtime.clone();
    runtime.spawn(async move {
        // space this out to prevent it from clashing against the queue-builder thread on launch
        tokio::time::sleep(Duration::from_secs(30)).await;

        watch_registry(&config, &context).await
    });

    Ok(())
}

pub fn start_daemon(context: Context) -> Result<(), Error> {
    let context = Arc::new(context);
    let runtime = context.runtime.clone();

    let web_config = Arc::new(docs_rs_web::Config::from_environment()?);
    let watcher_config = Arc::new(docs_rs_watcher::Config::from_environment()?);
    let builder_config = Arc::new(docs_rs_builder::Config::from_environment()?);

    // Start the web server before doing anything more expensive
    // Please check with an administrator before changing this (see #1172 for context).
    info!("Starting web server");
    let webserver_thread = thread::spawn({
        let context = context.clone();
        let runtime = runtime.clone();
        move || runtime.block_on(run_web_server(None, web_config, context))
    });

    // check new crates every minute
    start_registry_watcher(watcher_config.clone(), context.clone())?;

    // build new crates every minute
    let rustwide_builder = RustwideBuilder::init(builder_config.clone(), &context)?;

    thread::Builder::new()
        .name("build queue reader".to_string())
        .spawn({
            let context = context.clone();
            move || queue_builder(&context, &builder_config, rustwide_builder).unwrap()
        })
        .unwrap();

    runtime.block_on(async {
        start_background_repository_stats_updater(&context).await?;
        start_background_queue_rebuild(watcher_config, &context.clone()).await?;

        // when people run the daemon, we assume the daemon is the one single process where
        // we can collect the service metrics.
        start_background_service_metric_collector(&context).await?;

        Ok::<(), Error>(())
    })?;

    // NOTE: if a error occurred earlier in `start_daemon`, the server will _not_ be joined -
    // instead it will get killed when the process exits.
    webserver_thread
        .join()
        .map_err(|err| anyhow!("web server panicked: {:?}", err))?
}
