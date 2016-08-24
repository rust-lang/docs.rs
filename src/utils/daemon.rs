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


const DAEMON_PID_FILE_PATH: &'static str = "/var/run/cratesfyi.pid";


pub fn start_daemon() {
    // first check required environment variables
    for v in ["CRATESFYI_PREFIX",
              "CRATESFYI_PREFIX",
              "CRATESFYI_GITHUB_USERNAME",
              "CRATESFYI_GITHUB_ACCESSTOKEN"]
        .iter() {
        env::var(v).expect("Environment variable not found");
    }

    // check paths once
    opts().check_paths().unwrap();

    // fork the process
    let pid = unsafe { fork() };
    if pid > 0 {
        let mut file = File::create(DAEMON_PID_FILE_PATH).expect("Failed to create pid file");
        writeln!(&mut file, "{}", pid).expect("Failed to write pid");

        info!("cratesfyi {} daemon started on: {}", ::BUILD_VERSION, pid);
        exit(0);
    }


    // check new crates every 5 minutes
    thread::spawn(move || {
        loop {
            thread::sleep(Duration::from_secs(300));
            debug!("Checking new crates");
            let doc_builder = DocBuilder::new(opts());
            if let Err(e) = doc_builder.get_new_crates() {
                error!("Failed to get new crates: {}", e);
            }
        }
    });


    // build new crates every 3 minutes
    thread::spawn(move || {
        loop {
            thread::sleep(Duration::from_secs(180));

            let mut opts = opts();
            opts.skip_if_exists = true;

            // check lock file
            if opts.prefix.join("cratesfyi.lock").exists() {
                warn!("Lock file exits, skipping building new crates");
            }

            let mut doc_builder = DocBuilder::new(opts);
            if let Err(e) = doc_builder.load_cache() {
                error!("Failed to load cache: {}", e);
                continue;
            }

            debug!("Building new crates");
            if let Err(e) = doc_builder.build_packages_queue() {
                error!("Failed build new crates: {}", e);
            }

            if let Err(e) = doc_builder.save_cache() {
                error!("Failed to save cache: {}", e);
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
                if let Err(e) = ::utils::update_release_activity() {
                    error!("Failed to update release activity: {}", e);
                }
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
