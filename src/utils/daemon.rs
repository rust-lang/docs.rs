//! Simple daemon
//!
//! This daemon will start web server, track new packages and build them

use crate::{
    db::Pool,
    docbuilder::RustwideBuilder,
    utils::{github_updater, pubsubhubbub, update_release_activity},
    Config, DocBuilder, DocBuilderOptions,
};
use chrono::{Timelike, Utc};
use failure::Error;
use log::{debug, error, info, warn};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use std::{env, thread};

pub fn start_daemon(config: Arc<Config>, db: Pool) -> Result<(), Error> {
    const CRATE_VARIABLES: [&str; 3] = [
        "CRATESFYI_PREFIX",
        "CRATESFYI_GITHUB_USERNAME",
        "CRATESFYI_GITHUB_ACCESSTOKEN",
    ];

    // first check required environment variables
    for v in CRATE_VARIABLES.iter() {
        if env::var(v).is_err() {
            panic!("Environment variable {} not found", v)
        }
    }

    let dbopts = opts();

    // check paths once
    dbopts.check_paths().unwrap();

    // check new crates every minute
    let cloned_db = db.clone();
    thread::Builder::new()
        .name("registry index reader".to_string())
        .spawn(move || {
            // space this out to prevent it from clashing against the queue-builder thread on launch
            thread::sleep(Duration::from_secs(30));
            loop {
                let opts = opts();
                let mut doc_builder = DocBuilder::new(opts, cloned_db.clone());

                if doc_builder.is_locked() {
                    debug!("Lock file exists, skipping checking new crates");
                } else {
                    debug!("Checking new crates");
                    match doc_builder.get_new_crates() {
                        Ok(n) => debug!("{} crates added to queue", n),
                        Err(e) => error!("Failed to get new crates: {}", e),
                    }
                }

                thread::sleep(Duration::from_secs(60));
            }
        })
        .unwrap();

    // build new crates every minute
    // REFACTOR: Break this into smaller functions
    let cloned_db = db.clone();
    thread::Builder::new().name("build queue reader".to_string()).spawn(move || {
        let opts = opts();
        let mut doc_builder = DocBuilder::new(opts, cloned_db.clone());

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

        let mut builder = RustwideBuilder::init(cloned_db).unwrap();

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
                    Ok(n) => debug!("Succesfully pinged {} hubs", n)
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
            match doc_builder.get_queue_count() {
                Err(e) => {
                    error!("Failed to read the number of crates in the queue: {}", e);
                    continue;
                }

                Ok(0) => {
                    if status.count() > 0 {
                        // ping the hubs before continuing
                        match pubsubhubbub::ping_hubs() {
                            Err(e) => error!("Failed to ping hub: {}", e),
                            Ok(n) => debug!("Succesfully pinged {} hubs", n)
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
                    info!("Starting build with {} crates in queue (currently on a {} crate streak)",
                          queue_count, status.count());
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
                    Ok(crate_built) => if crate_built {
                        status.increment();
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
    }).unwrap();

    // update release activity everyday at 23:55
    let cloned_db = db.clone();
    cron(
        "release activity updater",
        Duration::from_secs(60),
        move || {
            let now = Utc::now();
            if now.hour() == 23 && now.minute() == 55 {
                info!("Updating release activity");
                update_release_activity(&*cloned_db.get()?)?;
            }
            Ok(())
        },
    )?;

    // update github stats every 6 hours
    let cloned_db = db.clone();
    cron(
        "github stats updater",
        Duration::from_secs(60 * 60 * 6),
        move || {
            github_updater(&*cloned_db.get()?)?;
            Ok(())
        },
    )?;

    // TODO: update ssl certificate every 3 months

    // at least start web server
    info!("Starting web server");

    crate::Server::start(None, false, db, config)?;
    Ok(())
}

fn cron<F>(name: &'static str, interval: Duration, exec: F) -> Result<(), Error>
where
    F: Fn() -> Result<(), Error> + Send + 'static,
{
    thread::Builder::new()
        .name(name.into())
        .spawn(move || loop {
            thread::sleep(interval);
            if let Err(err) = exec() {
                error!("failed to run scheduled task '{}': {:?}", name, err);
            }
        })?;
    Ok(())
}

fn opts() -> DocBuilderOptions {
    let prefix = PathBuf::from(
        env::var("CRATESFYI_PREFIX").expect("CRATESFYI_PREFIX environment variable not found"),
    );
    DocBuilderOptions::from_prefix(prefix)
}
