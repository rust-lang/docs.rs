use crate::{Context, db::delete, utils::spawn_blocking};
use anyhow::{Context as _, Result};
use itertools::Itertools;
use tracing::{info, warn};

mod data;
mod db;
mod diff;
mod index;

const BUILD_PRIORITY: i32 = 15;

/// consistency check
///
/// will compare our database with the local crates.io index and
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
pub async fn run_check<C: Context>(ctx: &C, dry_run: bool) -> Result<()> {
    let index = ctx.index()?;

    info!("Loading data from database...");
    let mut conn = ctx.async_pool().await?.get_async().await?;
    let db_data = db::load(&mut conn, &*ctx.config()?)
        .await
        .context("Loading crate data from database for consistency check")?;

    tracing::info!("Loading data from index...");
    let index_data = spawn_blocking({
        let index = index.clone();
        move || index::load(&index)
    })
    .await
    .context("Loading crate data from index for consistency check")?;

    let diff = diff::calculate_diff(db_data.iter(), index_data.iter());
    let result = handle_diff(ctx, diff.iter(), dry_run).await?;

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

    Ok(())
}

#[derive(Default)]
struct HandleResult {
    builds_queued: u32,
    crates_deleted: u32,
    releases_deleted: u32,
    yanks_corrected: u32,
}

async fn handle_diff<'a, I, C>(ctx: &C, iter: I, dry_run: bool) -> Result<HandleResult>
where
    I: Iterator<Item = &'a diff::Difference>,
    C: Context,
{
    let mut result = HandleResult::default();

    let config = ctx.config()?;

    let storage = ctx.async_storage().await?;
    let build_queue = ctx.async_build_queue().await?;

    let mut conn = ctx.async_pool().await?.get_async().await?;

    for difference in iter {
        println!("{difference}");

        match difference {
            diff::Difference::CrateNotInIndex(name) => {
                if !dry_run {
                    if let Err(err) = delete::delete_crate(&mut conn, &storage, &config, name).await
                    {
                        warn!("{:?}", err);
                    }
                }
                result.crates_deleted += 1;
            }
            diff::Difference::CrateNotInDb(name, versions) => {
                for version in versions {
                    if !dry_run {
                        if let Err(err) = build_queue
                            .add_crate(name, version, BUILD_PRIORITY, None)
                            .await
                        {
                            warn!("{:?}", err);
                        }
                    }
                    result.builds_queued += 1;
                }
            }
            diff::Difference::ReleaseNotInIndex(name, version) => {
                if !dry_run {
                    if let Err(err) =
                        delete::delete_version(&mut conn, &storage, &config, name, version).await
                    {
                        warn!("{:?}", err);
                    }
                }
                result.releases_deleted += 1;
            }
            diff::Difference::ReleaseNotInDb(name, version) => {
                if !dry_run {
                    if let Err(err) = build_queue
                        .add_crate(name, version, BUILD_PRIORITY, None)
                        .await
                    {
                        warn!("{:?}", err);
                    }
                }
                result.builds_queued += 1;
            }
            diff::Difference::ReleaseYank(name, version, yanked) => {
                if !dry_run {
                    if let Err(err) = build_queue.set_yanked(name, version, *yanked).await {
                        warn!("{:?}", err);
                    }
                }
                result.yanks_corrected += 1;
            }
        }
    }

    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::diff::Difference;
    use super::*;
    use crate::test::{TestEnvironment, async_wrapper};
    use sqlx::Row as _;

    async fn count(env: &TestEnvironment, sql: &str) -> Result<i64> {
        let mut conn = env.async_db().await.async_conn().await;
        Ok(sqlx::query_scalar(sql).fetch_one(&mut *conn).await?)
    }

    async fn single_row<O>(env: &TestEnvironment, sql: &str) -> Result<Vec<O>>
    where
        O: Send + Unpin + for<'r> sqlx::Decode<'r, sqlx::Postgres> + sqlx::Type<sqlx::Postgres>,
    {
        let mut conn = env.async_db().await.async_conn().await;
        Ok::<_, anyhow::Error>(
            sqlx::query(sql)
                .fetch_all(&mut *conn)
                .await?
                .into_iter()
                .map(|row| row.get(0))
                .collect(),
        )
    }

    #[test]
    fn test_delete_crate() {
        async_wrapper(|env| async move {
            env.fake_release()
                .await
                .name("krate")
                .version("0.1.1")
                .version("0.1.2")
                .create()
                .await?;

            let diff = [Difference::CrateNotInIndex("krate".into())];

            // calling with dry-run leads to no change
            handle_diff(&*env, diff.iter(), true).await?;

            assert_eq!(
                count(&env, "SELECT count(*) FROM crates WHERE name = 'krate'").await?,
                1
            );

            // without dry-run the crate will be deleted
            handle_diff(&*env, diff.iter(), false).await?;

            assert_eq!(
                count(&env, "SELECT count(*) FROM crates WHERE name = 'krate'").await?,
                0
            );

            Ok(())
        })
    }

    #[test]
    fn test_delete_release() {
        async_wrapper(|env| async move {
            env.fake_release()
                .await
                .name("krate")
                .version("0.1.1")
                .create()
                .await?;
            env.fake_release()
                .await
                .name("krate")
                .version("0.1.2")
                .create()
                .await?;

            let diff = [Difference::ReleaseNotInIndex(
                "krate".into(),
                "0.1.1".into(),
            )];

            assert_eq!(count(&env, "SELECT count(*) FROM releases").await?, 2);

            handle_diff(&*env, diff.iter(), true).await?;

            assert_eq!(count(&env, "SELECT count(*) FROM releases").await?, 2);

            handle_diff(&*env, diff.iter(), false).await?;

            assert_eq!(
                single_row::<String>(&env, "SELECT version FROM releases").await?,
                vec!["0.1.2"]
            );

            Ok(())
        })
    }

    #[test]
    fn test_wrong_yank() {
        async_wrapper(|env| async move {
            env.fake_release()
                .await
                .name("krate")
                .version("0.1.1")
                .yanked(true)
                .create()
                .await?;

            let diff = [Difference::ReleaseYank(
                "krate".into(),
                "0.1.1".into(),
                false,
            )];

            handle_diff(&*env, diff.iter(), true).await?;

            assert_eq!(
                single_row::<bool>(&env, "SELECT yanked FROM releases").await?,
                vec![true]
            );

            handle_diff(&*env, diff.iter(), false).await?;

            assert_eq!(
                single_row::<bool>(&env, "SELECT yanked FROM releases").await?,
                vec![false]
            );

            Ok(())
        })
    }

    #[test]
    fn test_missing_release_in_db() {
        async_wrapper(|env| async move {
            let diff = [Difference::ReleaseNotInDb("krate".into(), "0.1.1".into())];

            handle_diff(&*env, diff.iter(), true).await?;

            let build_queue = env.async_build_queue().await;

            assert!(build_queue.queued_crates().await?.is_empty());

            handle_diff(&*env, diff.iter(), false).await?;

            assert_eq!(
                build_queue
                    .queued_crates()
                    .await?
                    .iter()
                    .map(|c| (c.name.as_str(), c.version.as_str(), c.priority))
                    .collect::<Vec<_>>(),
                vec![("krate", "0.1.1", 15)]
            );
            Ok(())
        })
    }

    #[test]
    fn test_missing_crate_in_db() {
        async_wrapper(|env| async move {
            let diff = [Difference::CrateNotInDb(
                "krate".into(),
                vec!["0.1.1".into(), "0.1.2".into()],
            )];

            handle_diff(&*env, diff.iter(), true).await?;

            let build_queue = env.async_build_queue().await;

            assert!(build_queue.queued_crates().await?.is_empty());

            handle_diff(&*env, diff.iter(), false).await?;

            assert_eq!(
                build_queue
                    .queued_crates()
                    .await?
                    .iter()
                    .map(|c| (c.name.as_str(), c.version.as_str(), c.priority))
                    .collect::<Vec<_>>(),
                vec![("krate", "0.1.1", 15), ("krate", "0.1.2", 15)]
            );
            Ok(())
        })
    }
}
