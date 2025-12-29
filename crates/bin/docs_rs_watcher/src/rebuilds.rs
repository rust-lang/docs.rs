use crate::Config;
use anyhow::Result;
use chrono::NaiveDate;
use docs_rs_build_queue::{AsyncBuildQueue, PRIORITY_BROKEN_RUSTDOC, PRIORITY_CONTINUOUS};
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
    use crate::testing::TestEnvironment;
    use docs_rs_test_fakes::FakeBuild;
    use docs_rs_types::{
        BuildStatus,
        testing::{BAR, BAZ, FOO, V1, V2},
    };
    use pretty_assertions::assert_eq;

    #[tokio::test(flavor = "multi_thread")]
    async fn test_rebuild_when_old() -> Result<()> {
        let mut config = Config::test_config()?;
        config.max_queued_rebuilds = Some(100);
        let env = TestEnvironment::with_config(config).await?;

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

    /// Verifies whether a rebuild is queued for all releases with the latest build performed with a specific nightly version of rustdoc
    #[tokio::test(flavor = "multi_thread")]
    async fn test_rebuild_broken_rustdoc_specific_date_simple() -> Result<()> {
        let env = TestEnvironment::new().await?;

        // Matrix of test builds (crate name, nightly date, version)
        let build_matrix = [
            // Should be skipped since this is not the latest build for this release
            (FOO, NaiveDate::from_ymd_opt(2020, 10, 1).unwrap(), V1),
            // All those should match
            (FOO, NaiveDate::from_ymd_opt(2020, 10, 2).unwrap(), V1),
            (FOO, NaiveDate::from_ymd_opt(2020, 10, 2).unwrap(), V2),
            (BAR, NaiveDate::from_ymd_opt(2020, 10, 2).unwrap(), V1),
            // Should be skipped since the nightly doesn't match
            (BAR, NaiveDate::from_ymd_opt(2020, 10, 3).unwrap(), V2),
        ];
        for build in build_matrix.into_iter() {
            let (crate_name, nightly, version) = build;
            env.fake_release()
                .await
                .name(&crate_name)
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

        let build_queue = env.build_queue()?;
        assert!(build_queue.queued_crates().await?.is_empty());

        let mut conn = env.async_conn().await?;
        queue_rebuilds_faulty_rustdoc(
            &mut conn,
            build_queue,
            &NaiveDate::from_ymd_opt(2020, 10, 2).unwrap(),
            &None,
        )
        .await?;

        let queue = build_queue.queued_crates().await?;
        assert_eq!(queue.len(), 3);
        assert_eq!(queue[0].name, FOO);
        assert_eq!(queue[0].version, V1);
        assert_eq!(queue[0].priority, PRIORITY_BROKEN_RUSTDOC);
        assert_eq!(queue[1].name, FOO);
        assert_eq!(queue[1].version, V2);
        assert_eq!(queue[1].priority, PRIORITY_BROKEN_RUSTDOC);
        assert_eq!(queue[2].name, BAR);
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
            (FOO, NaiveDate::from_ymd_opt(2020, 10, 3).unwrap(), V1),
            // Should be skipped since the nightly doesn't match
            (FOO, NaiveDate::from_ymd_opt(2020, 10, 2).unwrap(), V1),
            // Should be skipped since the nightly doesn't match
            (BAR, NaiveDate::from_ymd_opt(2020, 10, 4).unwrap(), V1),
        ];
        for build in build_matrix.into_iter() {
            let (crate_name, nightly, version) = build;
            env.fake_release()
                .await
                .name(&crate_name)
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

        let build_queue = env.build_queue()?;
        assert!(build_queue.queued_crates().await?.is_empty());

        let mut conn = env.async_conn().await?;
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
            (FOO, NaiveDate::from_ymd_opt(2020, 10, 1).unwrap(), V1),
            // All those should match
            (FOO, NaiveDate::from_ymd_opt(2020, 10, 2).unwrap(), V1),
            (FOO, NaiveDate::from_ymd_opt(2020, 10, 3).unwrap(), V2),
            (BAR, NaiveDate::from_ymd_opt(2020, 10, 4).unwrap(), V1),
            // Should be skipped since the nightly doesn't match (end date is exclusive)
            (BAR, NaiveDate::from_ymd_opt(2020, 10, 5).unwrap(), V2),
        ];
        for build in build_matrix.into_iter() {
            let (crate_name, nightly, version) = build;
            env.fake_release()
                .await
                .name(&crate_name)
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

        let build_queue = env.build_queue()?;
        assert!(build_queue.queued_crates().await?.is_empty());

        let mut conn = env.async_conn().await?;
        queue_rebuilds_faulty_rustdoc(
            &mut conn,
            build_queue,
            &NaiveDate::from_ymd_opt(2020, 10, 2).unwrap(),
            &NaiveDate::from_ymd_opt(2020, 10, 5),
        )
        .await?;

        let queue = build_queue.queued_crates().await?;
        assert_eq!(queue.len(), 3);
        assert_eq!(queue[0].name, FOO);
        assert_eq!(queue[0].version, V1);
        assert_eq!(queue[0].priority, PRIORITY_BROKEN_RUSTDOC);
        assert_eq!(queue[1].name, FOO);
        assert_eq!(queue[1].version, V2);
        assert_eq!(queue[1].priority, PRIORITY_BROKEN_RUSTDOC);
        assert_eq!(queue[2].name, BAR);
        assert_eq!(queue[2].version, V1);
        assert_eq!(queue[2].priority, PRIORITY_BROKEN_RUSTDOC);

        Ok(())
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_still_rebuild_when_full_with_failed() -> Result<()> {
        let mut config = Config::test_config()?;
        config.max_queued_rebuilds = Some(1);
        let env = TestEnvironment::with_config(config).await?;

        let build_queue = env.build_queue()?;
        build_queue
            .add_crate(&FOO, &V1, PRIORITY_CONTINUOUS, None)
            .await?;
        build_queue
            .add_crate(&BAR, &V1, PRIORITY_CONTINUOUS, None)
            .await?;

        let mut conn = env.async_conn().await?;
        sqlx::query!("UPDATE queue SET attempt = 99")
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
        let env = TestEnvironment::with_config(config).await?;

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
