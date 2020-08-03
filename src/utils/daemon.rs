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
use std::sync::Arc;
use tokio::{
    runtime::Handle,
    sync::Mutex,
    task::{self, JoinHandle},
    time::{self, Duration, Instant},
};

async fn start_registry_watcher(
    pool: Pool,
    build_queue: Arc<BuildQueue>,
    options: DocBuilderOptions,
) -> JoinHandle<()> {
    task::spawn(async move {
        let (cloned_pool, cloned_build_queue, cloned_options) =
            (pool.clone(), build_queue.clone(), options.clone());

        let doc_builder = Arc::new(Mutex::new(
            task::spawn_blocking(move || {
                DocBuilder::new(cloned_options, cloned_pool, cloned_build_queue)
            })
            .await
            .unwrap(),
        ));

        loop {
            // Pause for a minute between each doc run
            time::delay_for(Duration::from_secs(60)).await;

            if doc_builder.lock().await.is_locked() {
                debug!("Lock file exists, skipping checking new crates");
            } else {
                debug!("Checking new crates");

                // TODO: When `.get_new_crates()` is async use `FutureExt::catch_unwind`
                // FIXME: Use `Result::flatten()` via https://github.com/rust-lang/rust/issues/70142
                let doc_builder = doc_builder.clone();
                match task::spawn_blocking(move || {
                    Handle::current()
                        .block_on(doc_builder.lock())
                        .get_new_crates()
                })
                .await
                .map_err(Into::into)
                {
                    Ok(Ok(n)) => debug!("{} crates added to queue", n),
                    Ok(Err(e)) | Err(e) => error!("Failed to get new crates: {}", e),
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
        start_registry_watcher(db.clone(), build_queue.clone(), dbopts.clone()).await;
    }

    // build new crates every minute
    let (cloned_db, cloned_build_queue, cloned_storage, options) = (
        db.clone(),
        build_queue.clone(),
        storage.clone(),
        dbopts.clone(),
    );
    task::spawn(async move {
        let (db, build_queue) = (cloned_db.clone(), cloned_build_queue.clone());
        let doc_builder = task::spawn_blocking(move || DocBuilder::new(options, db, build_queue))
            .await
            .unwrap();

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
            let db = cloned_db.clone();
            if let Err(err) = task::spawn_blocking(move || {
                db.get()
                    .map_err(Into::into)
                    .and_then(|mut conn| update_release_activity(&mut conn))
            })
            .await
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
