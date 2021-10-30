use crate::{docbuilder::RustwideBuilder, utils::report_error, BuildQueue};
use anyhow::Error;
use log::{debug, error, info, warn};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

// TODO: change to `fn() -> Result<!, Error>` when never _finally_ stabilizes
pub fn queue_builder(
    mut builder: RustwideBuilder,
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
        /// The builder has started (or just finished) building a crate.
        QueueInProgress,
    }

    let mut status = BuilderState::Fresh;

    loop {
        if !matches!(status, BuilderState::QueueInProgress) {
            thread::sleep(Duration::from_secs(60));
        }

        // check lock file
        if build_queue.is_locked() {
            warn!("Lock file exists, skipping building new crates");
            status = BuilderState::Locked;
            continue;
        }

        // Only build crates if there are any to build
        debug!("Checking build queue");
        match build_queue.pending_count() {
            Err(e) => {
                report_error(&e.context("Failed to read the number of crates in the queue"));
                continue;
            }

            Ok(0) => {
                debug!("Queue is empty, going back to sleep");
                status = BuilderState::EmptyQueue;
                continue;
            }

            Ok(queue_count) => info!("Starting build with {} crates in queue", queue_count),
        }

        status = BuilderState::QueueInProgress;

        // If a panic occurs while building a crate, lock the queue until an admin has a chance to look at it.
        let res = catch_unwind(AssertUnwindSafe(|| {
            if let Err(e) = build_queue.build_next_queue_package(&mut builder) {
                report_error(&e.context("Failed to build crate from queue"));
            }
        }));

        if let Err(e) = res {
            error!("GRAVE ERROR Building new crates panicked: {:?}", e);
            // If we panic here something is really truly wrong and trying to handle the error won't help.
            build_queue.lock().expect("failed to lock queue");
        }
    }
}
