//! Simple daemon
//!
//! This daemon will start web server, track new packages and build them

use crate::{
    utils::{queue_builder, update_release_activity, GithubUpdater},
    Context, DocBuilder, DocBuilderOptions,
};
use chrono::{Timelike, Utc};
use failure::Error;
use log::{debug, error, info};
use std::process::Command;
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

fn run_git_gc() {
    let gc = Command::new("git")
        .args(&["gc", "--auto"])
        .output();
        
    if let Err(err) = gc {
        log::error!("failed to run `git gc`: {:?}", err);
    }
}

fn start_registry_watcher(
    opts: DocBuilderOptions,
    pool: Pool,
    build_queue: Arc<BuildQueue>,
    config: Arc<Config>,
) -> Result<(), Error> {
    thread::Builder::new()
        .name("registry index reader".to_string())
        .spawn(move || {
            // space this out to prevent it from clashing against the queue-builder thread on launch
            thread::sleep(Duration::from_secs(30));
            run_git_gc();
            let mut last_gc = Instant::now();

            loop {
                let mut doc_builder =
                    DocBuilder::new(opts.clone(), pool.clone(), build_queue.clone());

                if doc_builder.is_locked() {
                    debug!("Lock file exists, skipping checking new crates");
                } else {
                    debug!("Checking new crates");
                    match doc_builder.get_new_crates() {
                        Ok(n) => debug!("{} crates added to queue", n),
                        Err(e) => error!("Failed to get new crates: {}", e),
                    }
                }

                if last_gc.elapsed().as_secs() >= config.registry_gc_interval {
                    run_git_gc();
                    last_gc = Instant::now();
                }
                thread::sleep(Duration::from_secs(60));
            }
        })?;

    Ok(())
}

pub fn start_daemon(context: &dyn Context, enable_registry_watcher: bool) -> Result<(), Error> {
    let config = context.config()?;
    let dbopts = DocBuilderOptions::new(config.prefix.clone(), config.registry_index_path.clone());

    // check paths once
    dbopts.check_paths().unwrap();

    if enable_registry_watcher {
        // check new crates every minute
        start_registry_watcher(
            dbopts.clone(),
            db.clone(),
            build_queue.clone(),
            config.clone(),
        )?;
    }

    // build new crates every minute
    let pool = context.pool()?;
    let build_queue = context.build_queue()?;
    let storage = context.storage()?;
    let metrics = context.metrics()?;
    thread::Builder::new()
        .name("build queue reader".to_string())
        .spawn(move || {
            let doc_builder = DocBuilder::new(dbopts.clone(), pool.clone(), build_queue.clone());
            queue_builder(doc_builder, pool, build_queue, metrics, storage).unwrap();
        })
        .unwrap();

    // update release activity everyday at 23:55
    let pool = context.pool()?;
    cron(
        "release activity updater",
        Duration::from_secs(60),
        move || {
            let now = Utc::now();
            if now.hour() == 23 && now.minute() == 55 {
                info!("Updating release activity");
                update_release_activity(&mut *pool.get()?)?;
            }
            Ok(())
        },
    )?;

    // update github stats every hour
    let github_updater = GithubUpdater::new(&config, context.pool()?)?;
    cron(
        "github stats updater",
        Duration::from_secs(60 * 60),
        move || {
            github_updater.update_all_crates()?;
            Ok(())
        },
    )?;

    // at least start web server
    info!("Starting web server");

    crate::Server::start(None, false, context)?;
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
