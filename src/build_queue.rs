use crate::{
    Config, Context, Index, PackageKind, RustwideBuilder,
    db::{delete_crate, delete_version},
    error::Result,
    utils::{get_crate_priority, report_error},
};
use anyhow::Context as _;
use chrono::NaiveDate;
use crates_index_diff::{Change, CrateVersion};
use docs_rs_build_queue::{
    AsyncBuildQueue, BuildPackageSummary, PRIORITY_BROKEN_RUSTDOC, PRIORITY_CONTINUOUS,
    PRIORITY_MANUAL_FROM_CRATES_IO, QueuedCrate,
};
use docs_rs_database::{
    crate_details::update_latest_version_id,
    service_config::{ConfigName, get_config, set_config},
};
use docs_rs_fastly::{Cdn, CdnBehaviour as _};
use docs_rs_types::{CrateId, KrateName, Version};
use docs_rs_utils::retry;
use fn_error_context::context;
use futures_util::StreamExt;
use std::time::Instant;
use tracing::{debug, error, info, instrument, warn};

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

async fn queue_crate_invalidation(krate: &str, cdn: Option<&Cdn>) {
    let krate = match krate
        .parse::<KrateName>()
        .with_context(|| format!("can't parse crate name '{}'", krate))
    {
        Ok(krate) => krate,
        Err(err) => {
            report_error(&err);
            return;
        }
    };

    let Some(cdn) = &cdn else {
        info!(%krate, "no CDN configured, skippping crate invalidation");
        return;
    };

    if let Err(err) = cdn.queue_crate_invalidation(&krate).await {
        report_error(&err);
    }
}

/// Updates registry index repository and adds new crates into build queue.
///
/// Returns the number of crates added
pub async fn get_new_crates(context: &Context, index: &Index) -> Result<usize> {
    let mut conn = context.pool.get_async().await?;

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
            Err(err) => report_error(&err),
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
    match change {
        Change::Added(release) => process_version_added(context, release, registry).await?,
        Change::AddedAndYanked(release) => {
            process_version_added(context, release, registry).await?;
            process_version_yank_status(context, release).await?;
        }
        Change::Unyanked(release) | Change::Yanked(release) => {
            process_version_yank_status(context, release).await?
        }
        Change::CrateDeleted { name, .. } => process_crate_deleted(context, name.as_str()).await?,
        Change::VersionDeleted(release) => process_version_deleted(context, release).await?,
    };
    Ok(change.added().is_some())
}

/// Processes crate changes, whether they got yanked or unyanked.
async fn process_version_yank_status(context: &Context, release: &CrateVersion) -> Result<()> {
    // FIXME: delay yanks of crates that have not yet finished building
    // https://github.com/rust-lang/docs.rs/issues/1934
    if let Ok(release_version) = Version::parse(&release.version) {
        set_yanked_inner(
            context,
            release.name.as_str(),
            &release_version,
            release.yanked,
        )
        .await?;
    }

    queue_crate_invalidation(&release.name, context.cdn.as_deref()).await;
    Ok(())
}

async fn process_version_added(
    context: &Context,
    release: &CrateVersion,
    registry: Option<&str>,
) -> Result<()> {
    let mut conn = context.pool.get_async().await?;
    let priority = get_crate_priority(&mut conn, &release.name).await?;
    let name: KrateName = release.name.parse()?;
    let version = &release
        .version
        .parse()
        .context("couldn't parse release version as semver")?;
    context
        .async_build_queue
        .add_crate(&name, version, priority, registry)
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
        .async_build_queue
        .deprioritize_other_releases(&name, version, PRIORITY_MANUAL_FROM_CRATES_IO)
        .await
        .unwrap_or_else(|err| report_error(&err));
    Ok(())
}

async fn process_version_deleted(context: &Context, release: &CrateVersion) -> Result<()> {
    let mut conn = context.pool.get_async().await?;

    let name: KrateName = release.name.parse()?;
    let version: Version = release
        .version
        .parse()
        .context("couldn't parse release version as semver")?;

    delete_version(
        &mut conn,
        &context.async_storage,
        &context.config,
        &release.name,
        &version,
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
    queue_crate_invalidation(&name, context.cdn.as_deref()).await;
    context
        .async_build_queue
        .remove_version_from_queue(&name, &version)
        .await?;
    Ok(())
}

async fn process_crate_deleted(context: &Context, krate: &str) -> Result<()> {
    let mut conn = context.pool.get_async().await?;

    delete_crate(&mut conn, &context.async_storage, &context.config, krate)
        .await
        .with_context(|| format!("failed to delete crate {krate}"))?;
    info!(
        name=%krate,
        "crate deleted from the index and the database",
    );
    queue_crate_invalidation(krate, context.cdn.as_deref()).await;

    let name: KrateName = krate.parse()?;
    context
        .async_build_queue
        .remove_crate_from_queue(&name)
        .await
}

pub async fn set_yanked(
    context: &Context,
    name: &str,
    version: &Version,
    yanked: bool,
) -> Result<()> {
    set_yanked_inner(context, name, version, yanked).await
}

#[context("error trying to set {name}-{version} to yanked: {yanked}")]
async fn set_yanked_inner(
    context: &Context,
    name: &str,
    version: &Version,
    yanked: bool,
) -> Result<()> {
    let mut conn = context.pool.get_async().await?;

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
        name,
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
        let name: KrateName = name.parse()?;
        match context
            .async_build_queue
            .has_build_queued(&name, version)
            .await
            .context("error trying to fetch build queue")
        {
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
                report_error(&err);
            }
        }
    }

    Ok(())
}

/// wrapper around BuildQueue::process_next_crate to handle metrics and cdn invalidation
fn process_next_crate(
    context: &Context,
    f: impl FnOnce(&QueuedCrate) -> Result<BuildPackageSummary>,
) -> Result<()> {
    let queue = context.build_queue.clone();
    let builder_metrics = context.builder_metrics.clone();
    let cdn = context.cdn.clone();
    let runtime = context.runtime.clone();
    let config = context.config.build_queue.clone();

    let next_attempt = queue.process_next_crate(|to_process| {
        let res = {
            let instant = Instant::now();
            let res = f(to_process);
            let elapsed = instant.elapsed().as_secs_f64();
            builder_metrics.build_time.record(elapsed, &[]);
            res
        };

        builder_metrics.total_builds.add(1, &[]);

        runtime.block_on(queue_crate_invalidation(&to_process.name, cdn.as_deref()));

        res
    })?;

    if let Some(next_attempt) = next_attempt
        && next_attempt >= config.build_attempts as i32
    {
        builder_metrics.failed_builds.add(1, &[]);
    }

    Ok(())
}

pub(crate) fn build_next_queue_package(
    context: &Context,
    builder: &mut RustwideBuilder,
) -> Result<bool> {
    let mut processed = false;

    let queue = context.build_queue.clone();

    process_next_crate(context, |krate| {
        processed = true;

        let kind = krate
            .registry
            .as_ref()
            .map(|r| PackageKind::Registry(r.as_str()))
            .unwrap_or(PackageKind::CratesIo);

        if let Err(err) = retry(
            || {
                builder
                    .reinitialize_workspace_if_interval_passed(context)
                    .context("Reinitialize workspace failed, locking queue")
            },
            3,
        ) {
            report_error(&err);
            queue.lock()?;
            return Err(err);
        }

        if let Err(err) = builder
            .update_toolchain_and_add_essential_files()
            .context("Updating toolchain failed, locking queue")
        {
            report_error(&err);
            queue.lock()?;
            return Err(err);
        }

        builder.build_package(&krate.name, &krate.version, kind, krate.attempt == 0)
    })?;

    Ok(processed)
}

/// Queue rebuilds as configured.
///
/// The idea is to rebuild:
/// * the latest release of each crate
/// * when the nightly version is older than our configured threshold
/// * and there was a successful build for that release, that included documentation.
/// * starting with the oldest nightly versions.
/// * also checking if there is already a build queued.
///
/// This might exclude releases from rebuilds that
/// * previously failed but would succeed with a newer nightly version
/// * previously failed but would succeed just with a retry.
#[instrument(skip_all)]
pub async fn queue_rebuilds(
    conn: &mut sqlx::PgConnection,
    config: &Config,
    queue: &AsyncBuildQueue,
) -> Result<()> {
    let already_queued_rebuilds: usize = queue
        .pending_count_by_priority()
        .await?
        .iter()
        .filter_map(|(priority, count)| (*priority >= PRIORITY_CONTINUOUS).then_some(count))
        .sum();

    let rebuilds_to_queue = config
        .max_queued_rebuilds
        .expect("config.max_queued_rebuilds not set") as i64
        - already_queued_rebuilds as i64;

    if rebuilds_to_queue <= 0 {
        info!("not queueing rebuilds; queue limit reached");
        return Ok(());
    }

    let mut results = sqlx::query!(
        r#"SELECT i.* FROM (
             SELECT
                 c.name as "name: KrateName",
                 r.version as "version: Version",
                 (
                    SELECT MAX(COALESCE(b.build_finished, b.build_started))
                    FROM builds AS b
                    WHERE b.rid = r.id
                 ) AS last_build_attempt
             FROM crates AS c
             INNER JOIN releases AS r ON c.latest_version_id = r.id

             WHERE
                 r.rustdoc_status = TRUE
         ) as i
         ORDER BY i.last_build_attempt ASC
         LIMIT $1"#,
        rebuilds_to_queue,
    )
    .fetch(&mut *conn);

    while let Some(row) = results.next().await {
        let row = row?;

        if !queue.has_build_queued(&row.name, &row.version).await? {
            info!("queueing rebuild for {} {}...", &row.name, &row.version);
            queue
                .add_crate(&row.name, &row.version, PRIORITY_CONTINUOUS, None)
                .await?;
        }
    }

    Ok(())
}

/// Queue rebuilds for failed crates due to a faulty version of rustdoc
///
/// It is assumed that the version of rustdoc matches the one of rustc, which is persisted in the DB.
/// The priority of the resulting rebuild requests will be lower than previously failed builds.
/// If a crate is already queued to be rebuilt, it will not be requeued.
/// Start date is inclusive, end date is exclusive.
#[instrument(skip_all)]
pub async fn queue_rebuilds_faulty_rustdoc(
    conn: &mut sqlx::PgConnection,
    queue: &AsyncBuildQueue,
    start_nightly_date: &NaiveDate,
    end_nightly_date: &Option<NaiveDate>,
) -> Result<i32> {
    let end_nightly_date =
        end_nightly_date.unwrap_or_else(|| start_nightly_date.succ_opt().unwrap());
    let mut results = sqlx::query!(
        r#"
        SELECT
            c.name AS "name: KrateName",
            r.version AS "version: Version"
        FROM crates AS c
        JOIN releases AS r
            ON c.id = r.crate_id
        JOIN release_build_status AS rbs
            ON rbs.rid = r.id
        JOIN builds AS b
            ON b.rid = r.id
            AND b.build_finished = rbs.last_build_time
            AND b.rustc_nightly_date >= $1
            AND b.rustc_nightly_date < $2
        "#,
        start_nightly_date,
        end_nightly_date
    )
    .fetch(&mut *conn);

    let mut results_count = 0;
    while let Some(row) = results.next().await {
        let row = row?;

        if !queue.has_build_queued(&row.name, &row.version).await? {
            results_count += 1;
            info!(
                name=%row.name,
                version=%row.version,
                priority=PRIORITY_BROKEN_RUSTDOC,
               "queueing rebuild"
            );
            queue
                .add_crate(&row.name, &row.version, PRIORITY_BROKEN_RUSTDOC, None)
                .await?;
        }
    }

    Ok(results_count)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test::{FakeBuild, TestEnvironment, V1, V2};
    use docs_rs_build_queue::{BuildPackageSummary, PRIORITY_DEFAULT};
    use docs_rs_headers::SurrogateKey;
    use docs_rs_types::{
        BuildStatus,
        testing::{BAR, FOO},
    };
    use pretty_assertions::assert_eq;

    #[tokio::test(flavor = "multi_thread")]
    async fn test_process_version_added() -> Result<()> {
        let env = TestEnvironment::new().await?;
        let build_queue = env.async_build_queue();

        let krate = CrateVersion {
            name: "krate".parse()?,
            version: V1.to_string().parse()?,
            ..Default::default()
        };
        process_version_added(&env.context, &krate, None).await?;
        let queue = build_queue.queued_crates().await?;
        assert_eq!(queue.len(), 1);
        assert_eq!(queue[0].priority, PRIORITY_DEFAULT);

        let krate = CrateVersion {
            name: "krate".parse()?,
            version: V2.to_string().parse()?,
            ..Default::default()
        };
        process_version_added(&env.context, &krate, None).await?;
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
        let mut conn = env.async_db().async_conn().await;

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
            name: "krate".parse()?,
            version: V1.to_string().parse()?,
            yanked: true,
            ..Default::default()
        };
        process_version_yank_status(&env.context, &krate).await?;

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
            name: "krate".parse()?,
            version: V1.to_string().parse()?,
            yanked: false,
            ..Default::default()
        };
        process_version_yank_status(&env.context, &krate).await?;

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
        let mut conn = env.async_db().async_conn().await;

        env.fake_release()
            .await
            .name("krate")
            .version(V1)
            .create()
            .await?;
        process_crate_deleted(&env.context, "krate").await?;

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
        let mut conn = env.async_db().async_conn().await;

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
            name: "krate".parse()?,
            version: V2.to_string().parse()?,
            ..Default::default()
        };
        process_version_deleted(&env.context, &krate).await?;

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
            name: "krate1".parse()?,
            version: V1.to_string().parse()?,
            ..Default::default()
        };
        let krate2 = CrateVersion {
            name: "krate2".parse()?,
            version: V1.to_string().parse()?,
            ..Default::default()
        };
        let krate_already_present = CrateVersion {
            name: "krate_already_present".parse()?,
            version: V1.to_string().parse()?,
            ..Default::default()
        };
        let non_existing_krate = CrateVersion {
            name: "krate_already_present".parse()?,
            version: V2.to_string().parse()?,
            ..Default::default()
        };
        let added = process_changes(
            &env.context,
            &vec![
                Change::Added(krate1),                         // Should be added correctly
                Change::Added(krate2),                         // Should be added correctly
                Change::VersionDeleted(krate_already_present), // Should be deleted correctly, without affecting the returned counter
                Change::VersionDeleted(non_existing_krate), // Should error out, but the error should be handled gracefully
            ],
            None,
        )
        .await;

        assert_eq!(added, 2);
        Ok(())
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_rebuild_when_old() -> Result<()> {
        let env = TestEnvironment::with_config(
            TestEnvironment::base_config()
                .max_queued_rebuilds(Some(100))
                .build()?,
        )
        .await?;

        env.fake_release()
            .await
            .name("foo")
            .version(V1)
            .builds(vec![
                FakeBuild::default().rustc_version("rustc 1.84.0-nightly (e7c0d2750 2020-10-15)"),
            ])
            .create()
            .await?;

        let build_queue = env.async_build_queue();
        assert!(build_queue.queued_crates().await?.is_empty());

        let mut conn = env.async_db().async_conn().await;
        queue_rebuilds(&mut conn, env.config(), build_queue).await?;

        let queue = build_queue.queued_crates().await?;
        assert_eq!(queue.len(), 1);
        assert_eq!(queue[0].name, "foo");
        assert_eq!(queue[0].version, V1);
        assert_eq!(queue[0].priority, PRIORITY_CONTINUOUS);

        Ok(())
    }

    /// Verifies whether a rebuild is queued for all releases with the latest build performed with a specific nightly version of rustdoc
    #[tokio::test(flavor = "multi_thread")]
    async fn test_rebuild_broken_rustdoc_specific_date_simple() -> Result<()> {
        let env = TestEnvironment::new().await?;

        // Matrix of test builds (crate name, nightly date, version)
        let build_matrix = [
            // Should be skipped since this is not the latest build for this release
            ("foo1", NaiveDate::from_ymd_opt(2020, 10, 1).unwrap(), V1),
            // All those should match
            ("foo1", NaiveDate::from_ymd_opt(2020, 10, 2).unwrap(), V1),
            ("foo1", NaiveDate::from_ymd_opt(2020, 10, 2).unwrap(), V2),
            ("foo2", NaiveDate::from_ymd_opt(2020, 10, 2).unwrap(), V1),
            // Should be skipped since the nightly doesn't match
            ("foo2", NaiveDate::from_ymd_opt(2020, 10, 3).unwrap(), V2),
        ];
        for build in build_matrix.into_iter() {
            let (crate_name, nightly, version) = build;
            env.fake_release()
                .await
                .name(crate_name)
                .version(version)
                .builds(vec![
                    FakeBuild::default()
                        .rustc_version(
                            format!(
                                "rustc 1.84.0-nightly (e7c0d2750 {})",
                                nightly.format("%Y-%m-%d")
                            )
                            .as_str(),
                        )
                        .build_status(BuildStatus::Failure),
                ])
                .create()
                .await?;
        }

        let build_queue = env.async_build_queue();
        assert!(build_queue.queued_crates().await?.is_empty());

        let mut conn = env.async_db().async_conn().await;
        queue_rebuilds_faulty_rustdoc(
            &mut conn,
            build_queue,
            &NaiveDate::from_ymd_opt(2020, 10, 2).unwrap(),
            &None,
        )
        .await?;

        let queue = build_queue.queued_crates().await?;
        assert_eq!(queue.len(), 3);
        assert_eq!(queue[0].name, "foo1");
        assert_eq!(queue[0].version, V1);
        assert_eq!(queue[0].priority, PRIORITY_BROKEN_RUSTDOC);
        assert_eq!(queue[1].name, "foo1");
        assert_eq!(queue[1].version, V2);
        assert_eq!(queue[1].priority, PRIORITY_BROKEN_RUSTDOC);
        assert_eq!(queue[2].name, "foo2");
        assert_eq!(queue[2].version, V1);
        assert_eq!(queue[2].priority, PRIORITY_BROKEN_RUSTDOC);

        Ok(())
    }

    /// Verifies whether a rebuild is NOT queued for any crate if the nightly specified doesn't match any latest build of any release
    #[tokio::test(flavor = "multi_thread")]
    async fn test_rebuild_broken_rustdoc_specific_date_skipped() -> Result<()> {
        let env = TestEnvironment::new().await?;

        // Matrix of test builds (crate name, nightly date, version)
        let build_matrix = [
            // Should be skipped since this is not the latest build for this release even if the nightly matches
            ("foo1", NaiveDate::from_ymd_opt(2020, 10, 3).unwrap(), V1),
            // Should be skipped since the nightly doesn't match
            ("foo1", NaiveDate::from_ymd_opt(2020, 10, 2).unwrap(), V1),
            // Should be skipped since the nightly doesn't match
            ("foo2", NaiveDate::from_ymd_opt(2020, 10, 4).unwrap(), V1),
        ];
        for build in build_matrix.into_iter() {
            let (crate_name, nightly, version) = build;
            env.fake_release()
                .await
                .name(crate_name)
                .version(version)
                .builds(vec![
                    FakeBuild::default()
                        .rustc_version(
                            format!(
                                "rustc 1.84.0-nightly (e7c0d2750 {})",
                                nightly.format("%Y-%m-%d")
                            )
                            .as_str(),
                        )
                        .build_status(BuildStatus::Failure),
                ])
                .create()
                .await?;
        }

        let build_queue = env.async_build_queue();
        assert!(build_queue.queued_crates().await?.is_empty());

        let mut conn = env.async_db().async_conn().await;
        queue_rebuilds_faulty_rustdoc(
            &mut conn,
            build_queue,
            &NaiveDate::from_ymd_opt(2020, 10, 3).unwrap(),
            &None,
        )
        .await?;

        let queue = build_queue.queued_crates().await?;
        assert_eq!(queue.len(), 0);

        Ok(())
    }

    /// Verifies whether a rebuild is queued for all releases with the latest build performed with a nightly version between two dates
    #[tokio::test(flavor = "multi_thread")]
    async fn test_rebuild_broken_rustdoc_date_range() -> Result<()> {
        let env = TestEnvironment::new().await?;

        // Matrix of test builds (crate name, nightly date, version)
        let build_matrix = [
            // Should be skipped since this is not the latest build for this release
            ("foo1", NaiveDate::from_ymd_opt(2020, 10, 1).unwrap(), V1),
            // All those should match
            ("foo1", NaiveDate::from_ymd_opt(2020, 10, 2).unwrap(), V1),
            ("foo1", NaiveDate::from_ymd_opt(2020, 10, 3).unwrap(), V2),
            ("foo2", NaiveDate::from_ymd_opt(2020, 10, 4).unwrap(), V1),
            // Should be skipped since the nightly doesn't match (end date is exclusive)
            ("foo2", NaiveDate::from_ymd_opt(2020, 10, 5).unwrap(), V2),
        ];
        for build in build_matrix.into_iter() {
            let (crate_name, nightly, version) = build;
            env.fake_release()
                .await
                .name(crate_name)
                .version(version)
                .builds(vec![
                    FakeBuild::default()
                        .rustc_version(
                            format!(
                                "rustc 1.84.0-nightly (e7c0d2750 {})",
                                nightly.format("%Y-%m-%d")
                            )
                            .as_str(),
                        )
                        .build_status(BuildStatus::Failure),
                ])
                .create()
                .await?;
        }

        let build_queue = env.async_build_queue();
        assert!(build_queue.queued_crates().await?.is_empty());

        let mut conn = env.async_db().async_conn().await;
        queue_rebuilds_faulty_rustdoc(
            &mut conn,
            build_queue,
            &NaiveDate::from_ymd_opt(2020, 10, 2).unwrap(),
            &NaiveDate::from_ymd_opt(2020, 10, 5),
        )
        .await?;

        let queue = build_queue.queued_crates().await?;
        assert_eq!(queue.len(), 3);
        assert_eq!(queue[0].name, "foo1");
        assert_eq!(queue[0].version, V1);
        assert_eq!(queue[0].priority, PRIORITY_BROKEN_RUSTDOC);
        assert_eq!(queue[1].name, "foo1");
        assert_eq!(queue[1].version, V2);
        assert_eq!(queue[1].priority, PRIORITY_BROKEN_RUSTDOC);
        assert_eq!(queue[2].name, "foo2");
        assert_eq!(queue[2].version, V1);
        assert_eq!(queue[2].priority, PRIORITY_BROKEN_RUSTDOC);

        Ok(())
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_still_rebuild_when_full_with_failed() -> Result<()> {
        let env = TestEnvironment::with_config(
            TestEnvironment::base_config()
                .max_queued_rebuilds(Some(1))
                .build()?,
        )
        .await?;

        let build_queue = env.async_build_queue();
        build_queue
            .add_crate(&FOO, &V1, PRIORITY_CONTINUOUS, None)
            .await?;
        build_queue
            .add_crate(&BAR, &V1, PRIORITY_CONTINUOUS, None)
            .await?;

        let mut conn = env.async_db().async_conn().await;
        sqlx::query!("UPDATE queue SET attempt = 99")
            .execute(&mut *conn)
            .await?;

        assert_eq!(build_queue.queued_crates().await?.len(), 0);

        env.fake_release()
            .await
            .name("foo")
            .version(V1)
            .builds(vec![
                FakeBuild::default().rustc_version("rustc 1.84.0-nightly (e7c0d2750 2020-10-15)"),
            ])
            .create()
            .await?;

        let build_queue = env.async_build_queue();
        queue_rebuilds(&mut conn, env.config(), build_queue).await?;

        assert_eq!(build_queue.queued_crates().await?.len(), 1);

        Ok(())
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_dont_rebuild_when_full() -> Result<()> {
        let env = TestEnvironment::with_config(
            TestEnvironment::base_config()
                .max_queued_rebuilds(Some(1))
                .build()?,
        )
        .await?;

        let build_queue = env.async_build_queue();
        build_queue
            .add_crate(&"foo1".parse().unwrap(), &V1, PRIORITY_CONTINUOUS, None)
            .await?;
        build_queue
            .add_crate(&"foo2".parse().unwrap(), &V1, PRIORITY_CONTINUOUS, None)
            .await?;

        env.fake_release()
            .await
            .name("foo")
            .version(V1)
            .builds(vec![
                FakeBuild::default().rustc_version("rustc 1.84.0-nightly (e7c0d2750 2020-10-15)"),
            ])
            .create()
            .await?;

        let build_queue = env.async_build_queue();
        assert_eq!(build_queue.queued_crates().await?.len(), 2);

        let mut conn = env.async_db().async_conn().await;
        queue_rebuilds(&mut conn, env.config(), build_queue).await?;

        assert_eq!(build_queue.queued_crates().await?.len(), 2);

        Ok(())
    }

    #[test]
    fn test_invalidate_cdn_after_error() -> Result<()> {
        let env = TestEnvironment::new_with_runtime()?;

        let queue = env.build_queue();

        const WILL_FAIL: KrateName = KrateName::from_static("will_fail");

        queue.add_crate(&WILL_FAIL, &V1, 0, None)?;

        process_next_crate(&env.context, |krate| {
            assert_eq!(WILL_FAIL, krate.name);
            anyhow::bail!("simulate a failure");
        })?;

        assert_eq!(
            env.runtime()
                .block_on(env.cdn().unwrap().purged_keys())
                .unwrap(),
            SurrogateKey::from(WILL_FAIL).into()
        );

        Ok(())
    }

    #[test]
    fn test_invalidate_cdn_after_build() -> Result<()> {
        let env = TestEnvironment::new_with_runtime()?;

        let queue = env.build_queue();

        const WILL_SUCCEED: KrateName = KrateName::from_static("will_succeed");
        queue.add_crate(&WILL_SUCCEED, &V1, -1, None)?;

        process_next_crate(&env.context, |krate| {
            assert_eq!(WILL_SUCCEED, krate.name);
            Ok(BuildPackageSummary::default())
        })?;

        assert_eq!(
            env.runtime()
                .block_on(env.cdn().unwrap().purged_keys())
                .unwrap(),
            SurrogateKey::from(WILL_SUCCEED).into()
        );

        Ok(())
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_last_seen_reference_in_db() -> Result<()> {
        let env = TestEnvironment::new().await?;

        let mut conn = env.async_db().async_conn().await;
        let queue = env.async_build_queue();
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

        let mut conn = env.async_db().async_conn().await;
        set_config(&mut conn, ConfigName::LastSeenIndexReference, "invalid")
            .await
            .unwrap();

        assert!(last_seen_reference(&mut conn).await.is_err());

        Ok(())
    }
}
