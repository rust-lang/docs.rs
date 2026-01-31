use crate::Config;
use anyhow::Result;
use docs_rs_build_queue::{AsyncBuildQueue, PRIORITY_CONTINUOUS};
use docs_rs_types::{KrateName, Version};
use futures_util::StreamExt;
use tracing::{info, instrument};

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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::TestEnvironment;
    use docs_rs_config::AppConfig as _;
    use docs_rs_test_fakes::FakeBuild;
    use docs_rs_types::testing::{BAR, BAZ, FOO, V1};
    use pretty_assertions::assert_eq;

    #[tokio::test(flavor = "multi_thread")]
    async fn test_rebuild_when_old() -> Result<()> {
        let mut config = Config::test_config()?;
        config.max_queued_rebuilds = Some(100);
        let env = TestEnvironment::builder().config(config).build().await?;

        env.fake_release()
            .await
            .name(&FOO)
            .version(V1)
            .builds(vec![
                FakeBuild::default().rustc_version("rustc 1.84.0-nightly (e7c0d2750 2020-10-15)"),
            ])
            .create()
            .await?;

        let build_queue = env.build_queue()?;
        assert!(build_queue.queued_crates().await?.is_empty());

        let mut conn = env.async_conn().await?;
        queue_rebuilds(&mut conn, env.config(), build_queue).await?;

        let queue = build_queue.queued_crates().await?;
        assert_eq!(queue.len(), 1);
        assert_eq!(queue[0].name, FOO);
        assert_eq!(queue[0].version, V1);
        assert_eq!(queue[0].priority, PRIORITY_CONTINUOUS);

        Ok(())
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_still_rebuild_when_full_with_failed() -> Result<()> {
        let mut config = Config::test_config()?;
        config.max_queued_rebuilds = Some(1);
        let env = TestEnvironment::builder().config(config).build().await?;

        let build_queue = env.build_queue()?;
        build_queue
            .add_crate(&FOO, &V1, PRIORITY_CONTINUOUS, None)
            .await?;
        build_queue
            .add_crate(&BAR, &V1, PRIORITY_CONTINUOUS, None)
            .await?;

        let mut conn = env.async_conn().await?;
        sqlx::query!("DELETE FROM queue")
            .execute(&mut *conn)
            .await?;

        assert_eq!(build_queue.queued_crates().await?.len(), 0);

        env.fake_release()
            .await
            .name(&FOO)
            .version(V1)
            .builds(vec![
                FakeBuild::default().rustc_version("rustc 1.84.0-nightly (e7c0d2750 2020-10-15)"),
            ])
            .create()
            .await?;

        queue_rebuilds(&mut conn, env.config(), build_queue).await?;

        assert_eq!(build_queue.queued_crates().await?.len(), 1);

        Ok(())
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_dont_rebuild_when_full() -> Result<()> {
        let mut config = Config::test_config()?;
        config.max_queued_rebuilds = Some(1);
        let env = TestEnvironment::builder().config(config).build().await?;

        let build_queue = env.build_queue()?;
        build_queue
            .add_crate(&FOO.parse().unwrap(), &V1, PRIORITY_CONTINUOUS, None)
            .await?;
        build_queue
            .add_crate(&BAR.parse().unwrap(), &V1, PRIORITY_CONTINUOUS, None)
            .await?;

        env.fake_release()
            .await
            .name(&BAZ)
            .version(V1)
            .builds(vec![
                FakeBuild::default().rustc_version("rustc 1.84.0-nightly (e7c0d2750 2020-10-15)"),
            ])
            .create()
            .await?;

        let build_queue = env.build_queue()?;
        assert_eq!(build_queue.queued_crates().await?.len(), 2);

        let mut conn = env.async_conn().await?;
        queue_rebuilds(&mut conn, env.config(), build_queue).await?;

        assert_eq!(build_queue.queued_crates().await?.len(), 2);

        Ok(())
    }
}
