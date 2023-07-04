use crate::{db::delete, Context};
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
pub fn run_check(ctx: &dyn Context, dry_run: bool) -> Result<()> {
    let mut conn = ctx.pool()?.get()?;
    let index = ctx.index()?;

    info!("Loading data from database...");
    let db_data = db::load(&mut conn, &*ctx.config()?)
        .context("Loading crate data from database for consistency check")?;

    tracing::info!("Loading data from index...");
    let index_data =
        index::load(&index).context("Loading crate data from index for consistency check")?;

    let diff = diff::calculate_diff(db_data.iter(), index_data.iter());
    let result = handle_diff(ctx, diff.iter(), dry_run)?;

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

fn handle_diff<'a, I>(ctx: &dyn Context, iter: I, dry_run: bool) -> Result<HandleResult>
where
    I: Iterator<Item = &'a diff::Difference>,
{
    let mut result = HandleResult::default();

    let mut conn = ctx.pool()?.get()?;
    let storage = ctx.storage()?;
    let config = ctx.config()?;
    let build_queue = ctx.build_queue()?;

    for difference in iter {
        println!("{difference}");

        match difference {
            diff::Difference::CrateNotInIndex(name) => {
                if !dry_run {
                    if let Err(err) = delete::delete_crate(&mut conn, &storage, &config, name) {
                        warn!("{:?}", err);
                    }
                }
                result.crates_deleted += 1;
            }
            diff::Difference::CrateNotInDb(name, versions) => {
                for version in versions {
                    if !dry_run {
                        if let Err(err) = build_queue.add_crate(name, version, BUILD_PRIORITY, None)
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
                        delete::delete_version(&mut conn, &storage, &config, name, version)
                    {
                        warn!("{:?}", err);
                    }
                }
                result.releases_deleted += 1;
            }
            diff::Difference::ReleaseNotInDb(name, version) => {
                if !dry_run {
                    if let Err(err) = build_queue.add_crate(name, version, BUILD_PRIORITY, None) {
                        warn!("{:?}", err);
                    }
                }
                result.builds_queued += 1;
            }
            diff::Difference::ReleaseYank(name, version, yanked) => {
                if !dry_run {
                    if let Err(err) = build_queue.set_yanked(&mut conn, name, version, *yanked) {
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
    use postgres_types::FromSql;

    use super::diff::Difference;
    use super::*;
    use crate::test::{wrapper, TestEnvironment};

    fn count(env: &TestEnvironment, sql: &str) -> Result<i64> {
        Ok(env.db().conn().query_one(sql, &[])?.get::<_, i64>(0))
    }

    fn single_row<T>(env: &TestEnvironment, sql: &str) -> Result<Vec<T>>
    where
        T: for<'a> FromSql<'a>,
    {
        Ok(env
            .db()
            .conn()
            .query(sql, &[])?
            .iter()
            .map(|row| row.get::<_, T>(0))
            .collect())
    }

    #[test]
    fn test_delete_crate() {
        wrapper(|env| {
            env.fake_release()
                .name("krate")
                .version("0.1.1")
                .version("0.1.2")
                .create()?;

            let diff = vec![Difference::CrateNotInIndex("krate".into())];

            // calling with dry-run leads to no change
            handle_diff(env, diff.iter(), true)?;

            assert_eq!(
                count(env, "SELECT count(*) FROM crates WHERE name = 'krate'")?,
                1
            );

            // without dry-run the crate will be deleted
            handle_diff(env, diff.iter(), false)?;

            assert_eq!(
                count(env, "SELECT count(*) FROM crates WHERE name = 'krate'")?,
                0
            );

            Ok(())
        })
    }

    #[test]
    fn test_delete_release() {
        wrapper(|env| {
            env.fake_release().name("krate").version("0.1.1").create()?;
            env.fake_release().name("krate").version("0.1.2").create()?;

            let diff = vec![Difference::ReleaseNotInIndex(
                "krate".into(),
                "0.1.1".into(),
            )];

            assert_eq!(count(env, "SELECT count(*) FROM releases")?, 2);

            handle_diff(env, diff.iter(), true)?;

            assert_eq!(count(env, "SELECT count(*) FROM releases")?, 2);

            handle_diff(env, diff.iter(), false)?;

            assert_eq!(
                single_row::<String>(env, "SELECT version FROM releases")?,
                vec!["0.1.2"]
            );

            Ok(())
        })
    }

    #[test]
    fn test_wrong_yank() {
        wrapper(|env| {
            env.fake_release()
                .name("krate")
                .version("0.1.1")
                .yanked(true)
                .create()?;

            let diff = vec![Difference::ReleaseYank(
                "krate".into(),
                "0.1.1".into(),
                false,
            )];

            handle_diff(env, diff.iter(), true)?;

            assert_eq!(
                single_row::<bool>(env, "SELECT yanked FROM releases")?,
                vec![true]
            );

            handle_diff(env, diff.iter(), false)?;

            assert_eq!(
                single_row::<bool>(env, "SELECT yanked FROM releases")?,
                vec![false]
            );

            Ok(())
        })
    }

    #[test]
    fn test_missing_release_in_db() {
        wrapper(|env| {
            let diff = vec![Difference::ReleaseNotInDb("krate".into(), "0.1.1".into())];

            handle_diff(env, diff.iter(), true)?;

            let build_queue = env.build_queue();

            assert!(build_queue.queued_crates()?.is_empty());

            handle_diff(env, diff.iter(), false)?;

            assert_eq!(
                build_queue
                    .queued_crates()?
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
        wrapper(|env| {
            let diff = vec![Difference::CrateNotInDb(
                "krate".into(),
                vec!["0.1.1".into(), "0.1.2".into()],
            )];

            handle_diff(env, diff.iter(), true)?;

            let build_queue = env.build_queue();

            assert!(build_queue.queued_crates()?.is_empty());

            handle_diff(env, diff.iter(), false)?;

            assert_eq!(
                build_queue
                    .queued_crates()?
                    .iter()
                    .map(|c| (c.name.as_str(), c.version.as_str(), c.priority))
                    .collect::<Vec<_>>(),
                vec![("krate", "0.1.1", 15), ("krate", "0.1.2", 15)]
            );
            Ok(())
        })
    }
}
