//! Simple daemon
//!
//! This daemon will start web server, track new packages and build them

use crate::{
    db::Pool,
    storage::Storage,
    utils::{queue_builder, update_release_activity, GithubUpdater},
    BuildQueue, Config, DocBuilder, DocBuilderOptions,
};
use chrono::{Timelike, Utc};
use failure::Error;
use log::{debug, error, info};
use std::{env, path::PathBuf, sync::Arc};
use tokio::{
    task::{self, JoinHandle},
    time::{self, Duration, Instant},
};

async fn start_registry_watcher(
    opts: DocBuilderOptions,
    pool: Pool,
    build_queue: Arc<BuildQueue>,
) -> JoinHandle<()> {
    task::spawn(async move {
        let mut interval = time::interval(Duration::from_secs(60));

        let mut doc_builder = DocBuilder::new(opts.clone(), pool.clone(), build_queue.clone());

        loop {
            interval.tick().await;

            if doc_builder.is_locked() {
                debug!("Lock file exists, skipping checking new crates");
            } else {
                debug!("Checking new crates");

                match doc_builder.get_new_crates() {
                    Ok(n) => debug!("{} crates added to queue", n),
                    Err(e) => error!("Failed to get new crates: {}", e),
                }
            }
        }
    })
}

pub async fn start_daemon(
    config: Arc<Config>,
    db: Pool,
    build_queue: Arc<BuildQueue>,
    storage: Arc<Storage>,
    enable_registry_watcher: bool,
) -> Result<(), Error> {
    let dbopts = DocBuilderOptions::new(config.prefix.clone(), config.registry_index_path.clone());

    // check paths once
    dbopts.check_paths().unwrap();

    if enable_registry_watcher {
        // check new crates every minute
        start_registry_watcher(dbopts.clone(), db.clone(), build_queue.clone()).await;
    }

    // build new crates every minute
    let cloned_db = db.clone();
    let cloned_build_queue = build_queue.clone();
    let cloned_storage = storage.clone();

    task::spawn(async {
        let doc_builder = DocBuilder::new(
            dbopts.clone(),
            cloned_db.clone(),
            cloned_build_queue.clone(),
        );

        queue_builder(doc_builder, cloned_db, cloned_build_queue, cloned_storage)
            .await
            .unwrap();
    });

    // update release activity everyday at 23:55
    let cloned_db = db.clone();
    task::spawn(async move {
        // Calculate the Duration until 23:55
        let now = Utc::now();
        let until_midnight =
            Duration::from_secs(((23 - now.hour() as u64) * 60) + (55 - now.minute() as u64));
        let mut interval = time::interval_at(
            Instant::now() + until_midnight,
            Duration::from_secs(24 * 60 * 60),
        );

        loop {
            interval.tick().await;

            info!("Updating release activity");
            if let Err(err) = cloned_db
                .get()
                .map_err(Into::into)
                .and_then(|pool| update_release_activity(&mut *pool))
            {
                error!(
                    "failed to run scheduled task 'release activity updater': {:?}",
                    err
                );
            }
        }
    });

    // update github stats every hour
    let github_updater = GithubUpdater::new(&config, db.clone())?;
    task::spawn(async move {
        let mut interval = time::interval(Duration::from_secs(60 * 60));

        loop {
            interval.tick().await;

            if let Err(err) = github_updater.update_all_crates().await {
                error!(
                    "failed to run scheduled task 'github stats updater': {:?}",
                    err,
                );
            }
        }
    });

    // TODO: update ssl certificate every 3 months

    // at least start web server
    info!("Starting web server");

    crate::Server::start(None, false, db, config, build_queue, storage)?;
    Ok(())
}

fn opts() -> DocBuilderOptions {
    let prefix = PathBuf::from(
        env::var("CRATESFYI_PREFIX").expect("CRATESFYI_PREFIX environment variable not found"),
    );
    DocBuilderOptions::from_prefix(prefix)
}
