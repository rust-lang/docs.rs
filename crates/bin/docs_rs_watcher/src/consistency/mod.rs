use crate::{Config, Index, db::delete, index_watcher::set_yanked};
use anyhow::{Context as _, Result};
use chrono::{DateTime, Utc};
use docs_rs_build_queue::PRIORITY_CONSISTENCY_CHECK;
use docs_rs_context::Context;
use docs_rs_types::{KrateName, Version};
use itertools::Itertools;
use tracing::{info, warn};

mod data;
mod db;
mod diff;
mod index;

/// consistency check
///
/// will compare our database with the index at the watcher's last-seen commit
/// (or HEAD if no checkpoint exists) and
/// apply any changes that we find in the index but not our database.
///
/// Differences that we check for, and the activities:
/// * release in index, but not our DB => queue a build for this release.
/// * crate in index, but not in our DB => queue builds for all versions of that crate.
/// * release in DB, but not in the index => delete the release from our DB & storage.
/// * crate in our DB, but not in the index => delete the whole crate from our DB & storage.
/// * different yank-state between DB & Index => update the yank-state in our DB
///
/// Even when activities fail, the command can just be re-run. While the diff calculation will
/// be repeated, we won't re-execute fixing activities.
pub async fn run_check(config: &Config, ctx: &Context, dry_run: bool) -> Result<()> {
    let mut conn = ctx.pool()?.get_async().await?;
    let mut registry_index = Index::from_config(config, &mut conn).await?;
    let reference = registry_index.last_seen_reference().await?;

    info!(?reference, "Loading data from database...");
    let db_data = db::load(registry_index.connection())
        .await
        .context("Loading crate data from database for consistency check")?;

    let existing_crates = config
        .only_existing_crates
        .then(|| db_data.iter().map(|krate| krate.name.clone()).collect());

    tracing::info!("Loading data from index...");
    let index_data = index::load(&mut registry_index, existing_crates, reference)
        .await
        .context("Loading crate data from index for consistency check")?;

    let diff = diff::calculate_diff(db_data.iter(), index_data.iter());
    let result = handle_diff(config, ctx, diff.iter(), dry_run).await?;
    print_summary(&diff, &result, dry_run);

    registry_index.commit().await?;
    Ok(())
}

/// Check and resolve index inconsistencies for one explicitly selected crate.
/// This also queues missing crates, regardless of `only_existing_crates`.
pub async fn run_single_check(
    config: &Config,
    ctx: &Context,
    name: &KrateName,
    dry_run: bool,
) -> Result<()> {
    info!(%name, "Loading crate data from database...");
    let mut conn = ctx.pool()?.get_async().await?;
    let mut registry_index = Index::from_config(config, &mut conn).await?;
    let db_data = db::load_single(registry_index.connection(), name)
        .await
        .context("Loading crate data from database for consistency check")?;

    let reference = registry_index.last_seen_reference().await?;

    info!(%name, ?reference, "Loading crate data from Git index...");
    let index_data = index::load_single(&mut registry_index, name, reference)
        .await
        .context("Loading crate data from Git index for consistency check")?;

    let diff = diff::calculate_diff(db_data.iter(), index_data.iter());
    let result = handle_diff(config, ctx, diff.iter(), dry_run).await?;
    print_summary(&diff, &result, dry_run);

    registry_index.commit().await?;
    Ok(())
}

#[derive(Default)]
struct HandleResult {
    builds_queued: u32,
    crates_deleted: u32,
    releases_deleted: u32,
    yanks_corrected: u32,
    release_times_corrected: u32,
}

fn print_summary(diff: &[diff::Difference], result: &HandleResult, dry_run: bool) {
    println!("============");
    println!("SUMMARY");
    println!("============");
    println!("difference found:");
    for (key, count) in diff.iter().counts_by(|el| match el {
        diff::Difference::CrateNotInIndex(_) => "CrateNotInIndex",
        diff::Difference::CrateNotInDb(_, _) => "CrateNotInDb",
        diff::Difference::ReleaseNotInIndex(_, _) => "ReleaseNotInIndex",
        diff::Difference::ReleaseNotInDb(_, _) => "ReleaseNotInDb",
        diff::Difference::ReleaseYank(_, _, _) => "ReleaseYank",
        diff::Difference::ReleaseTime(_, _, _) => "ReleaseTime",
    }) {
        println!("{key:17} => {count:4}");
    }

    println!("============");
    if dry_run {
        println!("activities that would have been triggered:");
    } else {
        println!("activities triggered:");
    }
    println!("builds queued:    {:4}", result.builds_queued);
    println!("crates deleted:   {:4}", result.crates_deleted);
    println!("releases deleted: {:4}", result.releases_deleted);
    println!("yanks corrected:  {:4}", result.yanks_corrected);
    println!(
        "release times corrected: {:4}",
        result.release_times_corrected
    );
}

async fn handle_diff<'a, I>(
    config: &Config,
    ctx: &Context,
    iter: I,
    dry_run: bool,
) -> Result<HandleResult>
where
    I: Iterator<Item = &'a diff::Difference>,
{
    let mut result = HandleResult::default();

    let mut conn = ctx.pool()?.get_async().await?;

    for difference in iter {
        info!("{difference}");

        match difference {
            diff::Difference::CrateNotInIndex(name) => {
                if !dry_run
                    && let Err(err) =
                        delete::delete_crate(&mut conn, ctx.storage()?, config, name).await
                {
                    warn!(?difference, ?err, "error handling CrateNotInIndex");
                }
                result.crates_deleted += 1;
            }
            diff::Difference::CrateNotInDb(name, versions) => {
                for version in versions {
                    if !dry_run
                        && let Err(err) = ctx
                            .build_queue()?
                            .add_crate(name, version, PRIORITY_CONSISTENCY_CHECK)
                            .await
                    {
                        warn!(?difference, ?err, "error handling CrateNotInDb");
                    }
                    result.builds_queued += 1;
                }
            }
            diff::Difference::ReleaseNotInIndex(name, version) => {
                if !dry_run
                    && let Err(err) =
                        delete::delete_version(&mut conn, ctx.storage()?, config, name, version)
                            .await
                {
                    warn!(?difference, ?err, "error handling ReleaseNotInIndex");
                }
                result.releases_deleted += 1;
            }
            diff::Difference::ReleaseNotInDb(name, version) => {
                if !dry_run
                    && let Err(err) = ctx
                        .build_queue()?
                        .add_crate(name, version, PRIORITY_CONSISTENCY_CHECK)
                        .await
                {
                    warn!(?difference, ?err, "error handling ReleaseNotInDb");
                }
                result.builds_queued += 1;
            }
            diff::Difference::ReleaseYank(name, version, yanked) => {
                if !dry_run && let Err(err) = set_yanked(ctx, name, version, *yanked).await {
                    warn!(?difference, ?err, "error handling ReleaseYank ");
                }
                result.yanks_corrected += 1;
            }
            diff::Difference::ReleaseTime(name, version, release_time) => {
                if !dry_run
                    && let Err(err) =
                        set_release_time(&mut conn, name, version, *release_time).await
                {
                    warn!(?difference, ?err, "error handling ReleaseTime");
                }
                result.release_times_corrected += 1;
            }
        }
    }

    Ok(result)
}

async fn set_release_time(
    conn: &mut sqlx::PgConnection,
    name: &KrateName,
    version: &Version,
    release_time: DateTime<Utc>,
) -> Result<()> {
    sqlx::query!(
        r#"UPDATE releases
           SET release_time = $3
           FROM crates
           WHERE crates.id = releases.crate_id
             AND crates.name = $1
             AND releases.version = $2"#,
        name as _,
        version as _,
        release_time,
    )
    .execute(conn)
    .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::diff::Difference;
    use super::*;
    use crate::testing::{TestEnvironment, registry_history};
    use chrono::DateTime;
    use docs_rs_types::{
        Version,
        testing::{KRATE, V1, V2},
    };
    use sqlx::Row as _;

    async fn count(env: &TestEnvironment, sql: &'static str) -> Result<i64> {
        let mut conn = env.async_conn().await?;
        Ok(sqlx::query_scalar(sql).fetch_one(&mut *conn).await?)
    }

    async fn single_row<O>(env: &TestEnvironment, sql: &'static str) -> Result<Vec<O>>
    where
        O: Send + Unpin + for<'r> sqlx::Decode<'r, sqlx::Postgres> + sqlx::Type<sqlx::Postgres>,
    {
        let mut conn = env.async_conn().await?;
        Ok::<_, anyhow::Error>(
            sqlx::query(sql)
                .fetch_all(&mut *conn)
                .await?
                .into_iter()
                .map(|row| row.get(0))
                .collect(),
        )
    }

    #[test_case::test_case(false; "apply repairs")]
    #[test_case::test_case(true; "dry run")]
    #[tokio::test(flavor = "multi_thread")]
    async fn sync_uses_checkpoint(dry_run: bool) -> Result<()> {
        let env = TestEnvironment::new().await?;
        let (config, checkpoint, _) = registry_history(&env).await?;
        env.fake_release()
            .await
            .name(KRATE)
            .version(V1)
            .yanked(false)
            .create()
            .await?;
        let mut conn = env.async_conn().await?;
        let mut registry_index = Index::from_config(&config, &mut conn).await?;
        registry_index.set_last_seen_reference(checkpoint).await?;
        registry_index.commit().await?;

        let mut lock_conn = env.async_conn().await?;
        let registry_index = Index::from_config(&config, &mut lock_conn).await?;
        assert!(
            tokio::time::timeout(
                std::time::Duration::from_millis(100),
                run_check(&config, &env, dry_run),
            )
            .await
            .is_err()
        );
        assert_eq!(
            single_row::<bool>(&env, "SELECT yanked FROM releases").await?,
            vec![false]
        );
        registry_index.commit().await?;

        run_check(&config, &env, dry_run).await?;
        // HEAD unyanks V1 and publishes V2; neither change belongs to this sync.
        assert_eq!(
            single_row::<bool>(&env, "SELECT yanked FROM releases").await?,
            vec![!dry_run]
        );
        assert!(env.build_queue()?.queued_crates().await?.is_empty());
        let mut registry_index = Index::from_config(&config, &mut conn).await?;
        assert_eq!(
            registry_index.last_seen_reference().await?,
            Some(checkpoint)
        );
        registry_index.commit().await?;
        Ok(())
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn sync_uses_head_without_checkpoint() -> Result<()> {
        let env = TestEnvironment::new().await?;
        let (config, _, _) = registry_history(&env).await?;
        env.fake_release()
            .await
            .name(KRATE)
            .version(V1)
            .yanked(true)
            .create()
            .await?;

        run_check(&config, &env, false).await?;

        // HEAD unyanks V1 and publishes V2, unlike the older checkpoint.
        assert_eq!(
            single_row::<bool>(&env, "SELECT yanked FROM releases").await?,
            vec![false]
        );
        assert!(env.build_queue()?.has_build_queued(&KRATE, &V2).await?);
        let mut conn = env.async_conn().await?;
        let mut registry_index = Index::from_config(env.config(), &mut conn).await?;
        assert_eq!(registry_index.last_seen_reference().await?, None);
        registry_index.commit().await?;
        Ok(())
    }

    #[test_case::test_case(false; "HEAD without checkpoint")]
    #[test_case::test_case(true; "watcher checkpoint")]
    #[tokio::test(flavor = "multi_thread")]
    async fn single_sync_uses_git_snapshot(checkpoint_exists: bool) -> Result<()> {
        let env = TestEnvironment::new().await?;
        let (config, checkpoint, _) = registry_history(&env).await?;
        let mut conn = env.async_conn().await?;
        let mut registry_index = Index::from_config(&config, &mut conn).await?;
        let expected_reference = checkpoint_exists.then_some(checkpoint);
        if let Some(reference) = expected_reference {
            registry_index.set_last_seen_reference(reference).await?;
        }
        registry_index.commit().await?;

        run_single_check(&config, &env, &KRATE, false).await?;
        assert!(env.build_queue()?.has_build_queued(&KRATE, &V1).await?);
        assert_eq!(
            env.build_queue()?.has_build_queued(&KRATE, &V2).await?,
            !checkpoint_exists
        );
        let mut registry_index = Index::from_config(&config, &mut conn).await?;
        assert_eq!(
            registry_index.last_seen_reference().await?,
            expected_reference
        );
        registry_index.commit().await?;
        Ok(())
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_delete_crate() -> Result<()> {
        let env = TestEnvironment::new().await?;
        env.fake_release()
            .await
            .name("krate")
            .version(V1)
            .version(V2)
            .create()
            .await?;

        let diff = [Difference::CrateNotInIndex(KRATE)];

        // calling with dry-run leads to no change
        handle_diff(env.config(), &env, diff.iter(), true).await?;

        assert_eq!(
            count(&env, "SELECT count(*) FROM crates WHERE name = 'krate'").await?,
            1
        );

        // without dry-run the crate will be deleted
        handle_diff(env.config(), &env, diff.iter(), false).await?;

        assert_eq!(
            count(&env, "SELECT count(*) FROM crates WHERE name = 'krate'").await?,
            0
        );

        Ok(())
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_delete_release() -> Result<()> {
        let env = TestEnvironment::new().await?;
        env.fake_release()
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

        let diff = [Difference::ReleaseNotInIndex(KRATE, V1)];

        assert_eq!(count(&env, "SELECT count(*) FROM releases").await?, 2);

        handle_diff(env.config(), &env, diff.iter(), true).await?;

        assert_eq!(count(&env, "SELECT count(*) FROM releases").await?, 2);

        handle_diff(env.config(), &env, diff.iter(), false).await?;

        assert_eq!(
            single_row::<Version>(
                &env,
                r#"SELECT version as "version: Version" FROM releases"#
            )
            .await?,
            vec![V2]
        );

        Ok(())
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_wrong_yank() -> Result<()> {
        let env = TestEnvironment::new().await?;
        env.fake_release()
            .await
            .name("krate")
            .version(V1)
            .yanked(true)
            .create()
            .await?;

        let diff = [Difference::ReleaseYank(KRATE, V1, false)];

        handle_diff(env.config(), &env, diff.iter(), true).await?;

        assert_eq!(
            single_row::<bool>(&env, "SELECT yanked FROM releases").await?,
            vec![true]
        );

        handle_diff(env.config(), &env, diff.iter(), false).await?;

        assert_eq!(
            single_row::<bool>(&env, "SELECT yanked FROM releases").await?,
            vec![false]
        );

        Ok(())
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_wrong_release_time() -> Result<()> {
        let env = TestEnvironment::new().await?;
        let original = "2024-01-01T00:00:00Z".parse::<DateTime<Utc>>()?;
        let expected = "2024-01-02T00:00:00Z".parse::<DateTime<Utc>>()?;
        env.fake_release()
            .await
            .name("krate")
            .version(V1)
            .release_time(original)
            .create()
            .await?;

        let diff = [Difference::ReleaseTime(KRATE, V1, expected)];

        handle_diff(env.config(), &env, diff.iter(), true).await?;
        assert_eq!(
            single_row::<DateTime<Utc>>(&env, "SELECT release_time FROM releases").await?,
            vec![original]
        );

        handle_diff(env.config(), &env, diff.iter(), false).await?;
        assert_eq!(
            single_row::<DateTime<Utc>>(&env, "SELECT release_time FROM releases").await?,
            vec![expected]
        );

        Ok(())
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_missing_release_in_db() -> Result<()> {
        let env = TestEnvironment::new().await?;
        let diff = [Difference::ReleaseNotInDb(KRATE, V1)];

        handle_diff(env.config(), &env, diff.iter(), true).await?;

        let build_queue = env.build_queue()?;

        assert!(build_queue.queued_crates().await?.is_empty());

        handle_diff(env.config(), &env, diff.iter(), false).await?;

        assert_eq!(
            build_queue
                .queued_crates()
                .await?
                .into_iter()
                .map(|c| (c.name, V1, c.priority))
                .collect::<Vec<_>>(),
            vec![(KRATE, V1, 15)]
        );
        Ok(())
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_missing_crate_in_db() -> Result<()> {
        let env = TestEnvironment::new().await?;
        let diff = [Difference::CrateNotInDb(KRATE, vec![V1, V2])];

        handle_diff(env.config(), &env, diff.iter(), true).await?;

        let build_queue = env.build_queue()?;

        assert!(build_queue.queued_crates().await?.is_empty());

        handle_diff(env.config(), &env, diff.iter(), false).await?;

        assert_eq!(
            build_queue
                .queued_crates()
                .await?
                .into_iter()
                .map(|c| (c.name, c.version, c.priority))
                .collect::<Vec<_>>(),
            vec![(KRATE, V1, 15), (KRATE, V2, 15)]
        );
        Ok(())
    }
}
