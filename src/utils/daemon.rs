//! Simple daemon
//!
//! This daemon will start web server, track new packages and build them


use std::{env, thread};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::time::Duration;
use std::path::PathBuf;
use time;
use docbuilder::RustwideBuilder;
use DocBuilderOptions;
use DocBuilder;
use utils::{update_release_activity, github_updater, pubsubhubbub};
use db::{connect_db, update_search_index};

#[cfg(not(target_os = "windows"))]
use ::{
    libc::fork,
    std::process::exit,
    std::fs::File,
    std::io::Write
};

pub fn start_daemon(background: bool) {
    // first check required environment variables
    for v in ["CRATESFYI_PREFIX",
              "CRATESFYI_PREFIX",
              "CRATESFYI_GITHUB_USERNAME",
              "CRATESFYI_GITHUB_ACCESSTOKEN"]
        .iter() {
        env::var(v).expect(&format!("Environment variable {} not found", v));
    }

    let dbopts = opts();

    // check paths once
    dbopts.check_paths().unwrap();

    if background {
        #[cfg(target_os = "windows")] 
        {
            panic!("running in background not supported on windows");
        }
        #[cfg(not(target_os = "windows"))]
        {
            // fork the process
            let pid = unsafe { fork() };
            if pid > 0 {
                let mut file = File::create(dbopts.prefix.join("cratesfyi.pid"))
                    .expect("Failed to create pid file");
                writeln!(&mut file, "{}", pid).expect("Failed to write pid");

                info!("cratesfyi {} daemon started on: {}", ::BUILD_VERSION, pid);
                exit(0);
            }
        }
    }

    // check new crates every minute
    thread::Builder::new().name("crates.io reader".to_string()).spawn(move || {
        // space this out to prevent it from clashing against the queue-builder thread on launch
        thread::sleep(Duration::from_secs(30));
        loop {
            let opts = opts();
            let mut doc_builder = DocBuilder::new(opts);

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
    }).unwrap();

    // build new crates every minute
    thread::Builder::new().name("build queue reader".to_string()).spawn(move || {
        let opts = opts();
        let mut doc_builder = DocBuilder::new(opts);

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

        let mut builder = RustwideBuilder::init().unwrap();

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
    thread::Builder::new().name("release activity updater".to_string()).spawn(move || {
        loop {
            thread::sleep(Duration::from_secs(60));
            let now = time::now();
            if now.tm_hour == 23 && now.tm_min == 55 {
                info!("Updating release activity");
                if let Err(e) = update_release_activity() {
                    error!("Failed to update release activity: {}", e);
                }
            }
        }
    }).unwrap();


    // update search index every 3 hours
    thread::Builder::new().name("search index updater".to_string()).spawn(move || {
        loop {
            thread::sleep(Duration::from_secs(60 * 60 * 3));
            let conn = connect_db().expect("Failed to connect database");
            if let Err(e) = update_search_index(&conn) {
                error!("Failed to update search index: {}", e);
            }
        }
    }).unwrap();


    // update github stats every 6 hours
    thread::Builder::new().name("github stat updater".to_string()).spawn(move || {
        loop {
            thread::sleep(Duration::from_secs(60 * 60 * 6));
            if let Err(e) = github_updater() {
                error!("Failed to update github fields: {}", e);
            }
        }
    }).unwrap();

    // TODO: update ssl certificate every 3 months

    // at least start web server
    info!("Starting web server");
    ::Server::start(None);
}



fn opts() -> DocBuilderOptions {
    let prefix = PathBuf::from(env::var("CRATESFYI_PREFIX")
        .expect("CRATESFYI_PREFIX environment variable not found"));
    DocBuilderOptions::from_prefix(prefix)
}
