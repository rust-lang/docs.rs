use crate::{
    db::Pool, docbuilder::RustwideBuilder, utils::pubsubhubbub::HubPinger, BuildQueue, DocBuilder,
    Storage,
};
use failure::Error;
use log::{debug, error, info, warn};
use std::panic;
use std::sync::Arc;
use std::time::Duration;
use tokio::{runtime::Handle, sync::Mutex, task, time};

/// This is the core of the documentation building, within the inner loop we should never panic in any way
// TODO: change to `fn() -> Result<!, Error>` when never _finally_ stabilizes
pub async fn queue_builder(
    doc_builder: DocBuilder,
    db: Pool,
    build_queue: Arc<BuildQueue>,
    storage: Arc<Storage>,
) -> Result<(), Error> {
    // These are all locked up due to the cross-thread sharing currently involved, it may be able to be removed after the
    // build pipeline is async
    let doc_builder = Arc::new(Mutex::new(doc_builder));
    let builder = Arc::new(Mutex::new(
        task::spawn_blocking(|| RustwideBuilder::init(db, storage)).await??,
    ));
    let status = Arc::new(Mutex::new(BuilderState::Fresh));
    let hub_pinger = HubPinger::new();

    loop {
        if !status.lock().await.is_in_progress() {
            time::delay_for(Duration::from_secs(60)).await;
        }

        // check lock file
        if doc_builder.lock().await.is_locked() {
            warn!("Lock file exits, skipping building new crates");
            *status.lock().await = BuilderState::Locked;

            continue;
        }

        if status.lock().await.count() >= 10 {
            // periodically, we need to flush our caches and ping the hubs
            debug!("10 builds in a row; flushing caches");
            *status.lock().await = BuilderState::QueueInProgress(0);

            match hub_pinger.ping_hubs().await {
                Err(e) => error!("Failed to ping hub: {}", e),
                Ok(n) => debug!("Successfully pinged {} hubs", n),
            }

            if let Err(e) = doc_builder.lock().await.load_cache().await {
                error!("Failed to load cache: {}", e);
            }

            if let Err(e) = doc_builder.lock().await.save_cache().await {
                error!("Failed to save cache: {}", e);
            }
        }

        // Only build crates if there are any to build
        debug!("Checking build queue");
        let queue = Arc::clone(&build_queue);
        match task::spawn_blocking(move || queue.pending_count()).await? {
            Err(e) => {
                error!("Failed to read the number of crates in the queue: {}", e);
                continue;
            }

            Ok(0) => {
                if status.lock().await.count() > 0 {
                    // ping the hubs before continuing
                    match hub_pinger.ping_hubs().await {
                        Err(e) => error!("Failed to ping hub: {}", e),
                        Ok(n) => debug!("Succesfully pinged {} hubs", n),
                    }

                    if let Err(e) = doc_builder.lock().await.save_cache().await {
                        error!("Failed to save cache: {}", e);
                    }
                }

                debug!("Queue is empty, going back to sleep");
                *status.lock().await = BuilderState::EmptyQueue;

                continue;
            }

            Ok(queue_count) => {
                info!(
                    "Starting build with {} crates in queue (currently on a {} crate streak)",
                    queue_count,
                    status.lock().await.count()
                );
            }
        }

        // if we're starting a new batch, reload our caches and sources
        if !status.lock().await.is_in_progress() {
            if let Err(e) = doc_builder.lock().await.load_cache().await {
                error!("Failed to load cache: {}", e);

                continue;
            }
        }

        // Run build_packages_queue under `catch_unwind` to catch panics
        // This only panicked twice in the last 6 months but its just a better
        // idea to do this.
        let (doc_builder, status, builder) = (
            Arc::clone(&doc_builder),
            Arc::clone(&status),
            Arc::clone(&builder),
        );
        let res = task::spawn_blocking(move || {
            panic::catch_unwind(panic::AssertUnwindSafe(|| {
                match Handle::current()
                    .block_on(doc_builder.lock())
                    .build_next_queue_package(&mut *Handle::current().block_on(builder.lock()))
                {
                    Err(e) => error!("Failed to build crate from queue: {}", e),
                    Ok(crate_built) => {
                        if crate_built {
                            Handle::current().block_on(status.lock()).increment();
                        }
                    }
                }
            }))
        })
        .await;

        match res {
            Ok(Err(err)) => {
                error!("GRAVE ERROR Building new crates panicked: {:?}", err);
            }
            Err(err) => {
                error!("GRAVE ERROR Building new crates panicked: {:?}", err);
            }
            _ => {}
        }
    }
}

/// Represents the current state of the builder thread.
enum BuilderState {
    /// The builder thread has just started, and hasn't built any crates yet.
    Fresh,
    /// The builder has just seen an empty build queue.
    EmptyQueue,
    /// The builder has just seen the lock file.
    Locked,
    /// The builder has just finished building a crate. The enclosed count is the number of
    /// crates built since the caches have been refreshed.
    QueueInProgress(usize),
}

impl BuilderState {
    fn count(&self) -> usize {
        match *self {
            BuilderState::QueueInProgress(n) => n,
            _ => 0,
        }
    }

    fn is_in_progress(&self) -> bool {
        match *self {
            BuilderState::QueueInProgress(_) => true,
            _ => false,
        }
    }

    fn increment(&mut self) {
        *self = BuilderState::QueueInProgress(self.count() + 1);
    }
}
