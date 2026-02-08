use crate::{
    db::{delete_crate, delete_version},
    index::Index,
};
use anyhow::{Context as _, Result};
use crates_index_diff::Change;
use docs_rs_build_queue::{PRIORITY_MANUAL_FROM_CRATES_IO, priority::get_crate_priority};
use docs_rs_context::Context;
use docs_rs_database::{
    crate_details::update_latest_version_id,
    service_config::{ConfigName, get_config, set_config},
};
use docs_rs_fastly::{Cdn, CdnBehaviour as _};
use docs_rs_types::{CrateId, KrateName, Version};
use tracing::{debug, error, info, warn};

#[derive(Debug)]
pub(crate) struct CrateVersion {
    pub name: KrateName,
    pub version: Version,
    pub yanked: bool,
}

#[cfg(test)]
impl Default for CrateVersion {
    fn default() -> Self {
        Self {
            name: docs_rs_types::testing::KRATE,
            version: docs_rs_types::testing::V1,
            yanked: false,
        }
    }
}

impl TryFrom<crates_index_diff::CrateVersion> for CrateVersion {
    type Error = anyhow::Error;

    fn try_from(value: crates_index_diff::CrateVersion) -> Result<Self, Self::Error> {
        Ok(Self {
            name: value.name.parse()?,
            version: value.version.parse()?,
            yanked: value.yanked,
        })
    }
}

#[cfg(test)]
impl From<CrateVersion> for crates_index_diff::CrateVersion {
    fn from(value: CrateVersion) -> Self {
        Self {
            name: value.name.to_string().into(),
            version: value.version.to_string().into(),
            yanked: value.yanked,
            ..Default::default()
        }
    }
}

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

async fn queue_crate_invalidation(krate: &KrateName, cdn: Option<&Cdn>) {
    let Some(cdn) = &cdn else {
        info!(%krate, "no CDN configured, skippping crate invalidation");
        return;
    };

    if let Err(err) = cdn.queue_crate_invalidation(krate).await {
        error!(?krate, %err, "failed to queue crate invalidation");
    }
}

/// Updates registry index repository and adds new crates into build queue.
///
/// Returns the number of crates added
pub(crate) async fn get_new_crates(context: &Context, index: &Index) -> Result<usize> {
    let mut conn = context.pool()?.get_async().await?;

    let last_seen_reference = last_seen_reference(&mut conn).await?;
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

    debug!(last_seen_reference=%last_seen_reference, new_reference=%new_reference, "queueing changes");

    let crates_added = process_changes(context, &changes, index.repository_url()).await;

    if let Err(err) = context.build_queue()?.deprioritize_workspaces().await {
        error!(?err, "error deprioritizing workspaces");
    }

    // set the reference in the database
    // so this survives recreating the registry watcher
    // server.
    set_last_seen_reference(&mut conn, new_reference).await?;

    Ok(crates_added)
}

async fn process_changes(
    context: &Context,
    changes: &Vec<Change>,
    registry: Option<&str>,
) -> usize {
    let mut crates_added = 0;

    for change in changes {
        match process_change(context, change, registry).await {
            Ok(added) => {
                if added {
                    crates_added += 1;
                }
            }
            Err(err) => {
                error!(?change, ?err, "failed to process change");
            }
        }
    }
    crates_added
}

/// Process a crate change, returning whether the change was a crate addition or not.
async fn process_change(
    context: &Context,
    change: &Change,
    registry: Option<&str>,
) -> Result<bool> {
    let crate_version: CrateVersion = change
        .versions()
        .first()
        .expect("always exists")
        .clone()
        .try_into()?;

    match change {
        Change::Added(_release) => process_version_added(context, &crate_version, registry).await?,
        Change::AddedAndYanked(_release) => {
            process_version_added(context, &crate_version, registry).await?;
            process_version_yank_status(context, &crate_version).await?;
        }
        Change::Unyanked(_release) | Change::Yanked(_release) => {
            process_version_yank_status(context, &crate_version).await?
        }
        Change::CrateDeleted { name, .. } => {
            let name: KrateName = name.parse()?;
            process_crate_deleted(context, &name).await?
        }
        Change::VersionDeleted(_release) => {
            process_version_deleted(context, &crate_version).await?
        }
    };
    Ok(change.added().is_some())
}

/// Processes crate changes, whether they got yanked or unyanked.
async fn process_version_yank_status(context: &Context, release: &CrateVersion) -> Result<()> {
    // FIXME: delay yanks of crates that have not yet finished building
    // https://github.com/rust-lang/docs.rs/issues/1934
    set_yanked(context, &release.name, &release.version, release.yanked).await?;
    queue_crate_invalidation(&release.name, context.cdn.as_deref()).await;
    Ok(())
}

async fn process_version_added(
    context: &Context,
    release: &CrateVersion,
    registry: Option<&str>,
) -> Result<()> {
    let mut conn = context.pool()?.get_async().await?;
    let priority = get_crate_priority(&mut conn, &release.name).await?;
    context
        .build_queue()?
        .add_crate(&release.name, &release.version, priority, registry)
        .await
        .with_context(|| {
            format!(
                "failed adding {}-{} into build queue",
                release.name, release.version
            )
        })?;
    debug!(
        name=%release.name,
        version=%release.version,
        "added into build queue",
    );
    context
        .build_queue()?
        .deprioritize_other_releases(
            &release.name,
            &release.version,
            PRIORITY_MANUAL_FROM_CRATES_IO,
        )
        .await
        .unwrap_or_else(|err| error!(?err, "error deprioritizing older releases"));

    Ok(())
}

async fn process_version_deleted(context: &Context, release: &CrateVersion) -> Result<()> {
    let mut conn = context.pool()?.get_async().await?;

    delete_version(
        &mut conn,
        context.storage()?,
        &release.name,
        &release.version,
    )
    .await
    .with_context(|| {
        format!(
            "failed to delete version {}-{}",
            release.name, release.version
        )
    })?;
    info!(
        name=%release.name,
        version=%release.version,
        "release was deleted from the index and the database",
    );
    queue_crate_invalidation(&release.name, context.cdn.as_deref()).await;
    context
        .build_queue()?
        .remove_version_from_queue(&release.name, &release.version)
        .await?;
    Ok(())
}

async fn process_crate_deleted(context: &Context, krate: &KrateName) -> Result<()> {
    let mut conn = context.pool()?.get_async().await?;

    delete_crate(&mut conn, context.storage()?, krate)
        .await
        .with_context(|| format!("failed to delete crate {krate}"))?;
    info!(
        name=%krate,
        "crate deleted from the index and the database",
    );
    queue_crate_invalidation(krate, context.cdn.as_deref()).await;

    context.build_queue()?.remove_crate_from_queue(krate).await
}

pub(crate) async fn set_yanked(
    context: &Context,
    name: &KrateName,
    version: &Version,
    yanked: bool,
) -> Result<()> {
    let mut conn = context.pool()?.get_async().await?;

    let activity = if yanked { "yanked" } else { "unyanked" };

    if let Some(crate_id) = sqlx::query_scalar!(
        r#"UPDATE releases
         SET yanked = $3
         FROM crates
         WHERE crates.id = releases.crate_id
             AND name = $1
             AND version = $2
        RETURNING crates.id as "id: CrateId"
        "#,
        name as _,
        version as _,
        yanked,
    )
    .fetch_optional(&mut *conn)
    .await?
    {
        debug!(
            %name,
            %version,
            %activity,
            "updating latest version id"
        );
        update_latest_version_id(&mut conn, crate_id).await?;
    } else {
        match context.build_queue()?.has_build_queued(name, version).await {
            Ok(false) => {
                error!(
                    %name,
                    %version,
                    "tried to yank or unyank non-existing release",
                );
            }
            Ok(true) => {
                // the rustwide builder will fetch the current yank state from
                // crates.io, so and missed update here will be fixed after the
                // build is finished.
            }
            Err(err) => {
                error!(?err, "error trying to fetch build queue");
            }
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::TestEnvironment;
    use docs_rs_build_queue::PRIORITY_DEFAULT;
    use docs_rs_types::testing::{KRATE, V1, V2};
    use pretty_assertions::assert_eq;

    #[tokio::test(flavor = "multi_thread")]
    async fn test_process_version_added() -> Result<()> {
        let env = TestEnvironment::new().await?;
        let build_queue = env.build_queue()?;

        let krate = CrateVersion {
            name: KRATE,
            version: V1,
            ..Default::default()
        };

        process_version_added(&env, &krate, None).await?;
        let queue = build_queue.queued_crates().await?;
        assert_eq!(queue.len(), 1);
        assert_eq!(queue[0].priority, PRIORITY_DEFAULT);

        let krate = CrateVersion {
            name: "krate".parse()?,
            version: V2.to_string().parse()?,
            ..Default::default()
        };

        process_version_added(&env, &krate, None).await?;
        let queue = build_queue.queued_crates().await?;
        assert_eq!(queue.len(), 2);
        // The other queued version should be deprioritized
        assert_eq!(queue[0].version, V2);
        assert_eq!(queue[0].priority, PRIORITY_DEFAULT);
        assert_eq!(queue[1].version, V1);
        assert_eq!(queue[1].priority, PRIORITY_MANUAL_FROM_CRATES_IO);

        Ok(())
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_process_version_yank_status() -> Result<()> {
        let env = TestEnvironment::new().await?;
        let mut conn = env.async_conn().await?;

        // Given a release that is yanked
        let id = env
            .fake_release()
            .await
            .name("krate")
            .version(V1)
            .create()
            .await?;

        // Simulate a yank change
        let krate = CrateVersion {
            name: KRATE,
            version: V1,
            yanked: true,
        };
        process_version_yank_status(&env, &krate).await?;

        // And verify it's actually marked as yanked
        let row = sqlx::query!(
            "SELECT yanked
             FROM releases
             WHERE id = $1",
            id.0
        )
        .fetch_one(&mut *conn)
        .await?;
        assert_eq!(row.yanked, Some(true));

        // Verify whether we can unyank it too
        let krate = CrateVersion {
            name: KRATE,
            version: V1,
            yanked: false,
        };
        process_version_yank_status(&env, &krate).await?;

        let row = sqlx::query!(
            "SELECT yanked
             FROM releases
             WHERE id = $1",
            id.0
        )
        .fetch_one(&mut *conn)
        .await?;
        assert_eq!(row.yanked, Some(false));

        Ok(())
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_process_crate_deleted() -> Result<()> {
        let env = TestEnvironment::new().await?;
        let mut conn = env.async_conn().await?;

        env.fake_release()
            .await
            .name("krate")
            .version(V1)
            .create()
            .await?;

        process_crate_deleted(&env, &KRATE).await?;

        let row = sqlx::query!(
            "SELECT id
             FROM crates
             WHERE name = $1",
            "krate"
        )
        .fetch_optional(&mut *conn)
        .await?;
        assert!(row.is_none());
        Ok(())
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_process_version_deleted() -> Result<()> {
        let env = TestEnvironment::new().await?;
        let mut conn = env.async_conn().await?;

        let rid_1 = env
            .fake_release()
            .await
            .name("krate")
            .version(V1)
            .create()
            .await?;
        env.fake_release()
            .await
            .name("krate")
            .version(V2)
            .create()
            .await?;

        let krate = CrateVersion {
            name: KRATE,
            version: V2,
            ..Default::default()
        };
        process_version_deleted(&env, &krate).await?;

        let row = sqlx::query!(
            "SELECT id
             FROM releases",
        )
        .fetch_all(&mut *conn)
        .await?;
        assert_eq!(row.len(), 1);
        assert_eq!(row[0].id, rid_1.0);
        Ok(())
    }

    /// Ensure changes can be processed with graceful error handling and proper tracking of added versions
    #[tokio::test(flavor = "multi_thread")]
    async fn test_process_changes() -> Result<()> {
        let env = TestEnvironment::new().await?;

        env.fake_release()
            .await
            .name("krate_already_present")
            .version(V1)
            .create()
            .await?;

        let krate1 = CrateVersion {
            name: KRATE,
            version: V1,
            ..Default::default()
        };
        let krate2 = CrateVersion {
            name: "krate2".parse()?,
            version: V1,
            ..Default::default()
        };
        let krate_already_present = CrateVersion {
            name: "krate_already_present".parse()?,
            version: V1,
            ..Default::default()
        };
        let non_existing_version = CrateVersion {
            name: "krate_already_present".parse()?,
            version: V2,
            ..Default::default()
        };
        let added = process_changes(
            &env,
            &vec![
                // Should be added correctly
                Change::Added(krate1.into()),
                // Should be added correctly
                Change::Added(krate2.into()),
                // Should be deleted correctly, without affecting the returned counter
                Change::VersionDeleted(krate_already_present.into()),
                // Should error out, but the error should be handled gracefully
                Change::VersionDeleted(non_existing_version.into()),
            ],
            None,
        )
        .await;

        assert_eq!(added, 2);
        Ok(())
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_last_seen_reference_in_db() -> Result<()> {
        let env = TestEnvironment::new().await?;

        let mut conn = env.async_conn().await?;
        let queue = env.build_queue()?;
        queue.unlock().await?;
        assert!(!queue.is_locked().await?);

        // initial db ref is empty
        assert_eq!(last_seen_reference(&mut conn).await?, None);
        assert!(!queue.is_locked().await?);

        let oid = crates_index_diff::gix::ObjectId::from_hex(
            b"ffffffffffffffffffffffffffffffffffffffff",
        )?;
        set_last_seen_reference(&mut conn, oid).await?;

        assert_eq!(last_seen_reference(&mut conn).await?, Some(oid));
        assert!(!queue.is_locked().await?);

        Ok(())
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_broken_db_reference_breaks() -> Result<()> {
        let env = TestEnvironment::new().await?;

        let mut conn = env.async_conn().await?;
        set_config(&mut conn, ConfigName::LastSeenIndexReference, "invalid")
            .await
            .unwrap();

        assert!(last_seen_reference(&mut conn).await.is_err());

        Ok(())
    }
}
