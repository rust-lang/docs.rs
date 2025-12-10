mod build_queue;
mod config;
mod consistency;
pub(crate) mod db;
mod index;
mod priorities;
pub mod rebuilds;
pub mod repositories;
pub mod service_metrics;
mod utils;

pub use config::Config;

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
