use crate::{db::Pool, docbuilder::RustwideBuilder, utils::pubsubhubbub, BuildQueue, DocBuilder};
use failure::Error;
use log::{debug, error, info, warn};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

// TODO: change to `fn() -> Result<!, Error>` when never _finally_ stabilizes
// REFACTOR: Break this into smaller functions
pub fn queue_builder(
    mut doc_builder: DocBuilder,
    db: Pool,
    build_queue: Arc<BuildQueue>,
) -> Result<(), Error> {
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

    let mut builder = RustwideBuilder::init(db)?;

    let mut status = BuilderState::Fresh;

    loop {
        if !status.is_in_progress() {
            thread::sleep(Duration::from_secs(60));
        }

        // check lock file
        if doc_builder.is_locked() {
            warn!("Lock file exits, skipping building new crates");
            status = BuilderState::Locked;
            continue;
        }

        if status.count() >= 10 {
            // periodically, we need to flush our caches and ping the hubs
            debug!("10 builds in a row; flushing caches");
            status = BuilderState::QueueInProgress(0);

            match pubsubhubbub::ping_hubs() {
                Err(e) => error!("Failed to ping hub: {}", e),
                Ok(n) => debug!("Succesfully pinged {} hubs", n),
            }

            if let Err(e) = doc_builder.load_cache() {
                error!("Failed to load cache: {}", e);
            }

            if let Err(e) = doc_builder.save_cache() {
                error!("Failed to save cache: {}", e);
            }
        }

        // Only build crates if there are any to build
        debug!("Checking build queue");
        match build_queue.pending_count() {
            Err(e) => {
                error!("Failed to read the number of crates in the queue: {}", e);
                continue;
            }

            Ok(0) => {
                if status.count() > 0 {
                    // ping the hubs before continuing
                    match pubsubhubbub::ping_hubs() {
                        Err(e) => error!("Failed to ping hub: {}", e),
                        Ok(n) => debug!("Succesfully pinged {} hubs", n),
                    }

                    if let Err(e) = doc_builder.save_cache() {
                        error!("Failed to save cache: {}", e);
                    }
                }
                debug!("Queue is empty, going back to sleep");
                status = BuilderState::EmptyQueue;
                continue;
            }

            Ok(queue_count) => {
                info!(
                    "Starting build with {} crates in queue (currently on a {} crate streak)",
                    queue_count,
                    status.count()
                );
            }
        }

        // if we're starting a new batch, reload our caches and sources
        if !status.is_in_progress() {
            if let Err(e) = doc_builder.load_cache() {
                error!("Failed to load cache: {}", e);
                continue;
            }
        }

        // Run build_packages_queue under `catch_unwind` to catch panics
        // This only panicked twice in the last 6 months but its just a better
        // idea to do this.
        let res = catch_unwind(AssertUnwindSafe(|| {
            match doc_builder.build_next_queue_package(&mut builder) {
                Err(e) => error!("Failed to build crate from queue: {}", e),
                Ok(crate_built) => {
                    if crate_built {
                        status.increment();
                    }
                }
            }
        }));

        if let Err(e) = res {
            error!("GRAVE ERROR Building new crates panicked: {:?}", e);
        }
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
}
