//! Simple daemon
//!
//! This daemon will start web server, track new packages and build them

use crate::{Context, web::start_web_server};
use anyhow::{Error, anyhow};
use docs_rs_builder::{RustwideBuilder, queue_builder};
use docs_rs_context::Context as NewContext;
use docs_rs_watcher::{
    start_background_queue_rebuild, start_background_repository_stats_updater,
    start_background_service_metric_collector, watch_registry,
};
use std::sync::Arc;
use std::thread;
use std::time::Duration;
use tracing::info;

fn start_registry_watcher(
    config: Arc<docs_rs_watcher::Config>,
    context: Arc<NewContext>,
) -> Result<(), Error> {
    let runtime = context.runtime.clone();
    runtime.spawn(async move {
        // space this out to prevent it from clashing against the queue-builder thread on launch
        tokio::time::sleep(Duration::from_secs(30)).await;

        watch_registry(&config, &context).await
    });

    Ok(())
}

pub fn start_daemon(context: Context, enable_registry_watcher: bool) -> Result<(), Error> {
    let context = Arc::new(context);
    let runtime = context.runtime.clone();
    let new_context: Arc<docs_rs_context::Context> = Arc::new((&*context).into());

    // Start the web server before doing anything more expensive
    // Please check with an administrator before changing this (see #1172 for context).
    info!("Starting web server");
    let webserver_thread = thread::spawn({
        let context = context.clone();
        move || start_web_server(None, &context)
    });

    if enable_registry_watcher {
        // check new crates every minute
        start_registry_watcher(context.config.watcher.clone(), new_context.clone())?;
    }

    // build new crates every minute
    let builder_config = context.config.builder.clone();
    let new_context: docs_rs_context::Context = (&*context).into();
    let new_context = Arc::new(new_context);
    let rustwide_builder = RustwideBuilder::init(builder_config.clone(), &new_context)?;

    thread::Builder::new()
        .name("build queue reader".to_string())
        .spawn({
            let new_context = new_context.clone();
            move || queue_builder(&new_context, &builder_config, rustwide_builder).unwrap()
        })
        .unwrap();

    runtime.block_on(start_background_repository_stats_updater(&new_context))?;
    runtime.block_on(start_background_queue_rebuild(
        context.config.watcher.clone(),
        &new_context.clone(),
    ))?;

    // when people run the daemon, we assume the daemon is the one single process where
    // we can collect the service metrics.
    runtime.block_on(start_background_service_metric_collector(&new_context))?;

    // NOTE: if a error occurred earlier in `start_daemon`, the server will _not_ be joined -
    // instead it will get killed when the process exits.
    webserver_thread
        .join()
        .map_err(|err| anyhow!("web server panicked: {:?}", err))?
}
