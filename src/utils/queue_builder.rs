use crate::Context;
use crate::{docbuilder::RustwideBuilder, utils::report_error};
use anyhow::{Context as _, Error};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::time::Duration;
use std::{fs, io, path::Path, thread};
use tracing::{debug, error, warn};

/// the main build-server loop
pub fn queue_builder(context: &Context, mut builder: RustwideBuilder) -> Result<(), Error> {
    loop {
        let temp_dir = &context.config.temp_dir;
        if temp_dir.exists()
            && let Err(e) = remove_tempdirs(temp_dir)
        {
            report_error(&anyhow::anyhow!(e).context(format!(
                "failed to clean temporary directory {:?}",
                temp_dir
            )));
        }

        let build_queue = &context.build_queue;

        // check lock file
        match build_queue.is_locked().context("could not get queue lock") {
            Ok(true) => {
                warn!("Build queue is locked, skipping building new crates");
                thread::sleep(Duration::from_secs(60));
                continue;
            }
            Ok(false) => {}
            Err(err) => {
                report_error(&err);
                thread::sleep(Duration::from_secs(60));
                continue;
            }
        }

        // If a panic occurs while building a crate, lock the queue until an admin has a chance to look at it.
        debug!("Checking build queue");
        let res = catch_unwind(AssertUnwindSafe(|| {
            match build_queue.build_next_queue_package(context, &mut builder) {
                Ok(true) => {}
                Ok(false) => {
                    debug!("Queue is empty, going back to sleep");
                    thread::sleep(Duration::from_secs(60));
                }
                Err(e) => {
                    report_error(&e.context("Failed to build crate from queue"));
                }
            }
        }));

        if let Err(e) = res {
            error!("GRAVE ERROR Building new crates panicked: {:?}", e);
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
