use std::path::Path;

use crate::config::Config;
use crate::docbuilder::rustwide_builder::RustwideBuilder;
use anyhow::{Context as _, Result};
use docs_rs_context::Context;
use std::time::Duration;
use std::{fs, io, thread};
use tracing::{error, warn};

/// the main build-server loop
pub fn queue_builder(
    config: &Config,
    context: &Context,
    mut builder: RustwideBuilder,
) -> Result<()> {
    loop {
        let temp_dir = &config.temp_dir;
        if temp_dir.exists()
            && let Err(e) = remove_tempdirs(temp_dir)
        {
            error!(?e, temp_dir=%temp_dir.display(), "failed to clean temporary directory");
        }

        let build_queue = &context.blocking_build_queue()?;

        // check lock file
        match build_queue.is_locked().context("could not get queue lock") {
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

        //TODO: Build this with a new way

        // // If a panic occurs while building a crate, lock the queue until an admin has a chance to look at it.
        // debug!("Checking build queue");
        // let res = catch_unwind(AssertUnwindSafe(|| {
        //     match build_queue.build_next_queue_package(context, &mut builder) {
        //         Ok(true) => {}
        //         Ok(false) => {
        //             debug!("Queue is empty, going back to sleep");
        //             thread::sleep(Duration::from_secs(60));
        //         }
        //         Err(e) => {
        //             report_error(&e.context("Failed to build crate from queue"));
        //         }
        //     }
        // }));

        // if let Err(e) = res {
        //     error!("GRAVE ERROR Building new crates panicked: {:?}", e);
        //     thread::sleep(Duration::from_secs(60));
        //     continue;
        // }
        return Ok(());
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
