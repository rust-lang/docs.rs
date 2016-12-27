//! Simple daemon
//!
//! This daemon will start web server, track new packages and build them


use std::{env, thread};
use std::process::exit;
use std::fs::File;
use std::io::Write;
use std::time::Duration;
use std::path::PathBuf;
use libc::fork;
use time;
use DocBuilderOptions;
use DocBuilder;
use utils::{update_sources, update_release_activity, github_updater};
use db::{connect_db, update_search_index};



pub fn start_daemon() {
    // first check required environment variables
    for v in ["CRATESFYI_PREFIX",
              "CRATESFYI_PREFIX",
              "CRATESFYI_GITHUB_USERNAME",
              "CRATESFYI_GITHUB_ACCESSTOKEN"]
        .iter() {
        env::var(v).expect("Environment variable not found");
    }

    let dbopts = opts();

    // check paths once
    dbopts.check_paths().unwrap();

    // fork the process
    let pid = unsafe { fork() };
    if pid > 0 {
        let mut file = File::create(dbopts.prefix.join("cratesfyi.pid"))
            .expect("Failed to create pid file");
        writeln!(&mut file, "{}", pid).expect("Failed to write pid");

        info!("cratesfyi {} daemon started on: {}", ::BUILD_VERSION, pid);
        exit(0);
    }


    // check new crates every minute
    thread::spawn(move || {
        loop {
            thread::sleep(Duration::from_secs(60));

            let mut opts = opts();
            opts.skip_if_exists = true;

            // check lock file
            if opts.prefix.join("cratesfyi.lock").exists() {
                warn!("Lock file exits, skipping building new crates");
                continue;
            }

            let mut doc_builder = DocBuilder::new(opts);

            debug!("Checking new crates");
            let queue_count = match doc_builder.get_new_crates() {
                Ok(size) => size,
                Err(e) => {
                    error!("Failed to get new crates: {}", e);
                    continue;
                }
            };

            // Only build crates if there is any
            if queue_count == 0 {
                debug!("Queue is empty, going back to sleep");
                continue;
            }

            info!("Building {} crates from queue", queue_count);

            // update index
            if let Err(e) = update_sources() {
                error!("Failed to update sources: {}", e);
                continue;
            }

            if let Err(e) = doc_builder.load_cache() {
                error!("Failed to load cache: {}", e);
                continue;
            }


            // Run build_packages_queue in it's own thread to catch panics
            // This only panicked twice in the last 6 months but its just a better
            // idea to do this.
            let res = thread::spawn(move || {
                if let Err(e) = doc_builder.build_packages_queue() {
                    error!("Failed build new crates: {}", e);
                }

                if let Err(e) = doc_builder.save_cache() {
                    error!("Failed to save cache: {}", e);
                }

                debug!("Finished building new crates, going back to sleep");
            }).join();

            if let Err(e) = res {
                error!("GRAVE ERROR Building new crates panicked: {:?}", e);
            }
        }
    });


    // update release activity everyday at 02:00
    thread::spawn(move || {
        loop {
            thread::sleep(Duration::from_secs(60));
            let now = time::now();
            if now.tm_hour == 2 && now.tm_min == 0 {
                info!("Updating release activity");
                if let Err(e) = update_release_activity() {
                    error!("Failed to update release activity: {}", e);
                }
            }
        }
    });


    // update search index every 3 hours
    thread::spawn(move || {
        loop {
            thread::sleep(Duration::from_secs(60 * 60 * 3));
            let conn = connect_db().expect("Failed to connect database");
            if let Err(e) = update_search_index(&conn) {
                error!("Failed to update search index: {}", e);
            }
        }
    });


    // update github stats every 6 hours
    thread::spawn(move || {
        loop {
            thread::sleep(Duration::from_secs(60 * 60 * 6));
            if let Err(e) = github_updater() {
                error!("Failed to update github fields: {}", e);
            }
        }
    });

    // TODO: update ssl certificate every 3 months

    // at least start web server
    info!("Starting web server");
    ::start_web_server(None);
}



fn opts() -> DocBuilderOptions {
    let prefix = PathBuf::from(env::var("CRATESFYI_PREFIX")
        .expect("CRATESFYI_PREFIX environment variable not found"));
    DocBuilderOptions::from_prefix(prefix)
}
