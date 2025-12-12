use crate::config::Config;
use crate::docbuilder::rustwide_builder::{PackageKind, RustwideBuilder};
use anyhow::{Context as _, Result};
use docs_rs_build_queue::BuildQueue;
use docs_rs_context::Context;
use docs_rs_database::types::krate_name::KrateName;
use docs_rs_fastly::Cdn;
use docs_rs_utils::retry;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::Path;
use std::time::{Duration, Instant};
use std::{fs, io, thread};
use tracing::{debug, error, warn};

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

        // If a panic occurs while building a crate, lock the queue until an admin has a chance to look at it.
        debug!("Checking build queue");
        let res = catch_unwind(AssertUnwindSafe(|| {
            match build_next_queue_package(&build_queue, &mut builder) {
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
            error!("GRAVE ERROR Building new crates panicked: {:?}", e);
            thread::sleep(Duration::from_secs(60));
            continue;
        }
    }
}

/// Builds the top package from the queue. Returns whether there was a package in the queue.
///
/// Note that this will return `Ok(true)` even if the package failed to build.
fn build_next_queue_package(context: &Context, builder: &mut RustwideBuilder) -> Result<bool> {
    let build_queue = context.blocking_build_queue()?;
    let runtime = context.runtime();
    let cdn = context.cdn()?;
    let mut processed = false;

    let next_attempt = build_queue.process_next_crate(|krate| {
        processed = true;

        let kind = krate
            .registry
            .as_ref()
            .map(|r| PackageKind::Registry(r.as_str()))
            .unwrap_or(PackageKind::CratesIo);

        if let Err(err) = retry(|| builder.reinitialize_workspace_if_interval_passed(), 3) {
            error!(?err, "Reinitialize workspace failed, locking queue");
            build_queue.lock()?;
            return Err(err);
        }

        if let Err(err) = builder.update_toolchain_and_add_essential_files() {
            error!(?err, "Updating toolchain failed, locking queue");
            build_queue.lock()?;
            return Err(err);
        }

        let instant = Instant::now();
        let res = builder.build_package(&krate.name, &krate.version, kind, krate.attempt == 0);

        builder
            .builder_metrics
            .build_time
            .record(instant.elapsed().as_secs_f64(), &[]);
        builder.builder_metrics.total_builds.add(1, &[]);

        if let Ok(name) = krate.name.parse::<KrateName>() {
            runtime.block_on(cdn.queue_crate_invalidation(&name));
        }

        res
    })?;

    if let Some(attempt) = next_attempt {
        if attempt >= build_queue.config().build_attempts as i32 {
            builder.builder_metrics.failed_builds.add(1, &[]);
        }
    }

    Ok(processed)
}

/// Sometimes, when the server hits a hard crash or a build thread panics,
/// rustwide_builder won't actually remove the temporary directories it creates.
/// Remove them now to avoid running out of disk space.
fn remove_tempdirs<P: AsRef<Path>>(path: P) -> Result<(), io::Error> {
    fs::remove_dir_all(&path)?;
    fs::create_dir_all(&path)?;
    Ok(())
}
