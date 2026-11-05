use crate::build_queue::build_next_queue_package;
use crate::{Config, RustwideBuilder};
use anyhow::Result;
use docs_rs_context::Context;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::time::Duration;
use std::{fs, io, path::Path, thread};
use tracing::{debug, error, warn};

/// the main build-server loop
pub fn queue_builder(
    context: &Context,
    config: &Config,
    mut builder: RustwideBuilder,
) -> Result<()> {
    let build_queue = context.blocking_build_queue()?;

    loop {
        let temp_dir = &config.temp_dir;
        if temp_dir.exists()
            && let Err(e) = remove_tempdirs(temp_dir)
        {
            error!(temp_dir=%temp_dir.display(), ?e, "failed to clean temporary directory");
        }

        // Check whether starting new builds is paused.
        match build_queue.builds_locked() {
            Ok(true) => {
                warn!("Builds are locked, skipping building new crates");
                thread::sleep(Duration::from_mins(1));
                continue;
            }
            Ok(false) => {}
            Err(err) => {
                error!(?err, "could not get builds lock");
                thread::sleep(Duration::from_mins(1));
                continue;
            }
        }

        if let Some(min_queue_length) = config.min_queue_length {
            // This is a backlog threshold, not a global concurrency limit:
            // other builders may claim work after this count is read.
            match build_queue.processable_count() {
                Ok(count) if count < min_queue_length => {
                    debug!(
                        count,
                        min_queue_length, "Build queue below threshold, going back to sleep"
                    );
                    thread::sleep(Duration::from_mins(1));
                    continue;
                }
                Ok(_) => {}
                Err(err) => {
                    error!(?err, "Could not count processable builds");
                    thread::sleep(Duration::from_mins(1));
                    continue;
                }
            }
        }

        // Keep build panics from terminating the builder loop.
        debug!("Checking build queue");
        match catch_unwind(AssertUnwindSafe(|| {
            build_next_queue_package(context, &mut builder)
        })) {
            Ok(Ok(true)) => {}
            Ok(Ok(false)) => {
                debug!("No processable build, going back to sleep");
                thread::sleep(Duration::from_mins(1));
            }
            Ok(Err(e)) => {
                error!(?e, "Failed to build crate from queue");
            }
            Err(e) => {
                error!(?e, "GRAVE ERROR Building new crates panicked");
                thread::sleep(Duration::from_mins(1));
            }
        }
    }
}

/// Sometimes, when the server hits a hard crash or a build thread panics,
/// rustwide_builder won't actually remove the temporary directories it creates.
/// Remove them now to avoid running out of disk space.
fn remove_tempdirs<P: AsRef<Path>>(path: P) -> Result<(), io::Error> {
    fs::remove_dir_all(&path)?;
    fs::create_dir_all(&path)?;
    Ok(())
}
