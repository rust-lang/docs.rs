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

        // check lock file
        match build_queue.is_locked() {
            Ok(true) => {
                warn!("Build queue is locked, skipping building new crates");
                thread::sleep(Duration::from_secs(60));
                continue;
            }
            Ok(false) => {}
            Err(err) => {
                error!(?err, "could not get queue lock");
                thread::sleep(Duration::from_secs(60));
                continue;
            }
        }

        // If a panic occurs while building a crate, lock the queue until an admin has a chance to look at it.
        debug!("Checking build queue");
        let res = catch_unwind(AssertUnwindSafe(|| {
            match build_next_queue_package(context, &mut builder) {
                Ok(true) => {}
                Ok(false) => {
                    debug!("Queue is empty, going back to sleep");
                    thread::sleep(Duration::from_secs(60));
                }
                Err(e) => {
                    error!(?e, "Failed to build crate from queue");
                }
            }
        }));

        if let Err(e) = res {
            error!(?e, "GRAVE ERROR Building new crates panicked");
            thread::sleep(Duration::from_secs(60));
            continue;
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
