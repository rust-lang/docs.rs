use crate::Config;
use anyhow::Result;
use docs_rs_build_queue::{AsyncBuildQueue, PRIORITY_CONTINUOUS};
use docs_rs_database::types::version::Version;
use futures_util::StreamExt as _;
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
    build_queue: &AsyncBuildQueue,
) -> Result<()> {
    let already_queued_rebuilds: usize = build_queue
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
                 c.name,
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

        if !build_queue
            .has_build_queued(&row.name, &row.version)
            .await?
        {
            info!("queueing rebuild for {} {}...", &row.name, &row.version);
            build_queue
                .add_crate(&row.name, &row.version, PRIORITY_CONTINUOUS, None)
                .await?;
        }
    }

    Ok(())
}
