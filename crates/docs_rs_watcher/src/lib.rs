mod config;
mod consistency;
mod index;
mod utils;

use anyhow::Error;
use docs_rs_build_queue::AsyncBuildQueue;
use std::time::Instant;
use tracing::debug;

/// Run the registry watcher
/// NOTE: this should only be run once, otherwise crates would be added
/// to the queue multiple times.
pub async fn watch_registry(
    build_queue: &AsyncBuildQueue,
    config: &config::Config,
) -> Result<(), Error> {
    let mut last_gc = Instant::now();

    loop {
        if build_queue.is_locked().await? {
            debug!("Queue is locked, skipping checking new crates");
        } else {
            debug!("Checking new crates");
            let index = index::Index::from_config(config).await?;
            // TODO:
            // match build_queue
            //     .get_new_crates(&index)
            //     .await
            //     .context("Failed to get new crates")
            // {
            //     Ok(n) => debug!("{} crates added to queue", n),
            //     Err(e) => report_error(&e),
            // }

            if last_gc.elapsed().as_secs() >= config.registry_gc_interval {
                index.run_git_gc().await;
                last_gc = Instant::now();
            }
        }
        tokio::time::sleep(config.delay_between_registry_fetches).await;
    }
}

// async fn start_registry_watcher(
//     build_queue: &AsyncBuildQueue,
//     config: &config::Config,
// ) -> Result<(), Error> {
//     tokio::spawn(async move {
//         // space this out to prevent it from clashing against the queue-builder thread on launch
//         tokio::time::sleep(Duration::from_secs(30)).await;

//         watch_registry(&build_queue, &config).await
//     });

//     Ok(())
// }

pub async fn start_background_repository_stats_updater() -> Result<(), Error> {
    todo!();
    // // This call will still skip github repositories updates and continue if no token is provided
    // // (gitlab doesn't require to have a token). The only time this can return an error is when
    // // creating a pool or if config fails, which shouldn't happen here because this is run right at
    // // startup.
    // let updater = context.repository_stats_updater.clone();
    // let runtime = context.runtime.clone();
    // async_cron(
    //     &runtime,
    //     "repository stats updater",
    //     Duration::from_secs(60 * 60),
    //     move || {
    //         let updater = updater.clone();
    //         async move {
    //             updater.update_all_crates().await?;
    //             Ok(())
    //         }
    //     },
    // );
    // Ok(())
}

pub fn start_background_queue_rebuild() -> Result<(), Error> {
    todo!()

    // let runtime = context.runtime.clone();
    // let pool = context.pool.clone();
    // let config = context.config.clone();
    // let build_queue = context.async_build_queue.clone();

    // if config.max_queued_rebuilds.is_none() {
    //     info!("rebuild config incomplete, skipping rebuild queueing");
    //     return Ok(());
    // }

    // async_cron(
    //     &runtime,
    //     "background queue rebuilder",
    //     Duration::from_secs(60 * 60),
    //     move || {
    //         let pool = pool.clone();
    //         let build_queue = build_queue.clone();
    //         let config = config.clone();
    //         async move {
    //             let mut conn = pool.get_async().await?;
    //             queue_rebuilds(&mut conn, &config, &build_queue).await?;
    //             Ok(())
    //         }
    //     },
    // );
    // Ok(())
}
