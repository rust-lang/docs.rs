use crate::{docbuilder::RustwideBuilder, utils::report_error, BuildQueue};
use anyhow::{Context, Error};
use log::{debug, error, warn};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::Arc;
use std::time::Duration;
use std::{fs, io, thread};

pub(crate) const TEMPDIR_PREFIX: &str = "docsrs-docs";

pub fn queue_builder(
    mut builder: RustwideBuilder,
    build_queue: Arc<BuildQueue>,
) -> Result<(), Error> {
    loop {
        if let Err(e) = remove_tempdirs() {
            report_error(&anyhow::anyhow!(e).context("failed to remove temporary directories"));
        }

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
            match build_queue.build_next_queue_package(&mut builder) {
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
fn remove_tempdirs() -> Result<(), io::Error> {
    // NOTE: hardcodes that `tempfile::tempdir()` uses `std::env::temp_dir`.
    for entry in std::fs::read_dir(std::env::temp_dir())? {
        let entry = entry?;
        if !entry.metadata()?.is_dir() {
            continue;
        }

        if let Some(dir_name) = entry.path().file_name() {
            if dir_name.to_string_lossy().starts_with(TEMPDIR_PREFIX) {
                fs::remove_dir_all(entry.path())?;
            }
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remove_existing_tempdirs() {
        let file_with_prefix = tempfile::Builder::new()
            .prefix(TEMPDIR_PREFIX)
            .tempfile()
            .unwrap();

        let dir_with_prefix = tempfile::Builder::new()
            .prefix(TEMPDIR_PREFIX)
            .tempdir()
            .unwrap();

        let file_inside = dir_with_prefix.path().join("some_file_name");
        fs::File::create(&file_inside).unwrap();

        let other_file = tempfile::Builder::new().tempfile().unwrap();

        let other_dir = tempfile::Builder::new().tempdir().unwrap();

        assert!(dir_with_prefix.path().exists());

        remove_tempdirs().unwrap();

        assert!(!dir_with_prefix.path().exists());
        assert!(!file_inside.exists());

        // all these still exist
        assert!(file_with_prefix.path().exists());
        assert!(other_file.path().exists());
        assert!(other_dir.path().exists());
    }
}
