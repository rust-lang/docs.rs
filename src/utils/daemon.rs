//! Simple daemon
//!
//! This daemon will start web server, track new packages and build them


use std::env;
use libc::fork;
use std::process::exit;
use std::fs::File;
use std::io::Write;
use std::thread;
use std::time::Duration;
use std::path::PathBuf;
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

    info!("Starting cratesfyi {} daemon", ::BUILD_VERSION);

    // fork the process
    let pid = unsafe { fork() };
    if pid > 0 {
        let mut file = File::create(DAEMON_PID_FILE_PATH).expect("Failed to create pid file");
        writeln!(&mut file, "{}", pid).expect("Failed to write pid");

        info!("cratesfyi daemon started on: {}", pid);
        exit(0);
    }


    fn opts() -> DocBuilderOptions {
        let prefix = PathBuf::from(env::var("CRATESFYI_PREFIX")
                                   .expect("CRATESFYI_PREFIX environment variable not found"));
        let opts = DocBuilderOptions::from_prefix(prefix);
        opts.check_paths().unwrap();
        opts
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
            debug!("Building new crates");
            let mut doc_builder = DocBuilder::new(opts());
            if let Err(e) = doc_builder.build_packages_queue() {
                error!("Failed build new crates: {}", e);
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
