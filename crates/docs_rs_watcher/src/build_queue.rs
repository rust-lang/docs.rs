use crate::{
    db::delete::{delete_crate, delete_version},
    index::Index,
    priorities::get_crate_priority,
};
use anyhow::{Context as _, Result};
use docs_rs_build_queue::AsyncBuildQueue;
use docs_rs_database::{
    service_config::{ConfigName, get_config, set_config},
    types::{krate_name::KrateName, version::Version},
};
use docs_rs_fastly::Cdn;
use docs_rs_storage::AsyncStorage;
use tracing::{debug, error, info, warn};

pub async fn last_seen_reference(
    conn: &mut sqlx::PgConnection,
) -> Result<Option<crates_index_diff::gix::ObjectId>> {
    if let Some(value) = get_config::<String>(conn, ConfigName::LastSeenIndexReference).await? {
        return Ok(Some(crates_index_diff::gix::ObjectId::from_hex(
            value.as_bytes(),
        )?));
    }
    Ok(None)
}

pub async fn set_last_seen_reference(
    conn: &mut sqlx::PgConnection,
    oid: crates_index_diff::gix::ObjectId,
) -> Result<()> {
    set_config(conn, ConfigName::LastSeenIndexReference, oid.to_string()).await?;
    Ok(())
}

pub async fn get_new_crates(
    conn: &mut sqlx::PgConnection,
    index: &Index,
    build_queue: &AsyncBuildQueue,
    storage: &AsyncStorage,
    cdn: &Cdn,
) -> Result<usize> {
    let last_seen_reference = last_seen_reference(conn).await?;
    let last_seen_reference = if let Some(oid) = last_seen_reference {
        oid
    } else {
        warn!(
            "no last-seen reference found in our database. We assume a fresh install and
             set the latest reference (HEAD) as last. This means we will then start to queue
             builds for new releases only from now on, and not for all existing releases."
        );
        index.latest_commit_reference().await?
    };

    index.set_last_seen_reference(last_seen_reference).await?;

    let (changes, new_reference) = index.peek_changes_ordered().await?;

    let mut crates_added = 0;

    debug!("queueing changes from {last_seen_reference} to {new_reference}");

    for change in &changes {
        if let Some((ref krate, ..)) = change.crate_deleted() {
            match delete_crate(&mut conn, &storage, krate).await {
                Ok(_) => info!(
                    "crate {} was deleted from the index and the database",
                    krate
                ),
                Err(err) => {
                    // FIXME: worth going back to report_error here?
                    error!(?err, krate, "failed to delete crate");
                }
            };

            let krate: KrateName = krate.parse().unwrap();

            cdn.queue_crate_invalidation(&krate).await;
            build_queue.remove_crate_from_queue(&krate).await?;
            continue;
        }

        if let Some(release) = change.version_deleted() {
            let version: Version = release
                .version
                .parse()
                .context("couldn't parse release version as semver")?;

            match delete_version(&mut conn, &storage, &release.name, &version).await {
                Ok(_) => info!(
                    "release {}-{} was deleted from the index and the database",
                    release.name, release.version
                ),
                Err(err) => {
                    error!(?err, %release.name, %release.version, "failed to delete version")
                }
            }

            let krate: KrateName = release.name.parse().unwrap();
            cdn.queue_crate_invalidation(&krate).await;
            build_queue
                .remove_version_from_queue(&release.name, &version)
                .await?;
            continue;
        }

        if let Some(release) = change.added() {
            let priority = get_crate_priority(&mut conn, &release.name).await?;

            match build_queue
                .add_crate(
                    &release.name,
                    &release
                        .version
                        .parse()
                        .context("couldn't parse release version as semver")?,
                    priority,
                    index.repository_url(),
                )
                .await
            {
                Ok(()) => {
                    debug!(
                        "{}-{} added into build queue",
                        release.name, release.version
                    );
                    crates_added += 1;
                }
                Err(err) => {
                    error!(?err, %release.name, %release.version, "failed adding release build queue");
                }
            }
        }

        let yanked = change.yanked();
        let unyanked = change.unyanked();
        if let Some(release) = yanked.or(unyanked) {
            // FIXME: delay yanks of crates that have not yet finished building
            // https://github.com/rust-lang/docs.rs/issues/1934
            if let Ok(release_version) = Version::parse(&release.version)
                && let Err(err) = set_yanked_inner(
                    &mut conn,
                    release.name.as_str(),
                    &release_version,
                    yanked.is_some(),
                )
                .await
            {
                error!(?err, %release.name, %release.version, "error setting yanked status");
            }

            cdn.queue_crate_invalidation(&release.name).await;
        }
    }

    // set the reference in the database
    // so this survives recreating the registry watcher
    // server.
    set_last_seen_reference(conn, new_reference).await?;

    Ok(crates_added)
}
