use crate::{AsyncBuildQueue, Config, PRIORITY_BROKEN_RUSTDOC, PRIORITY_CONTINUOUS};
use anyhow::Result;
use chrono::NaiveDate;
use docs_rs_database::types::version::Version;
use futures_util::StreamExt as _;
use tracing::{info, instrument};

/// Queue rebuilds for failed crates due to a faulty version of rustdoc
///
/// It is assumed that the version of rustdoc matches the one of rustc, which is persisted in the DB.
/// The priority of the resulting rebuild requests will be lower than previously failed builds.
/// If a crate is already queued to be rebuilt, it will not be requeued.
/// Start date is inclusive, end date is exclusive.
#[instrument(skip_all)]
pub async fn queue_rebuilds_faulty_rustdoc(
    conn: &mut sqlx::PgConnection,
    build_queue: &AsyncBuildQueue,
    start_nightly_date: &NaiveDate,
    end_nightly_date: &Option<NaiveDate>,
) -> Result<i32> {
    let end_nightly_date =
        end_nightly_date.unwrap_or_else(|| start_nightly_date.succ_opt().unwrap());
    let mut results = sqlx::query!(
        r#"
         SELECT c.name,
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

        if !build_queue
            .has_build_queued(&row.name, &row.version)
            .await?
        {
            results_count += 1;
            info!(
                name=%row.name,
                version=%row.version,
                priority=PRIORITY_BROKEN_RUSTDOC,
               "queueing rebuild"
            );
            build_queue
                .add_crate(&row.name, &row.version, PRIORITY_BROKEN_RUSTDOC, None)
                .await?;
        }
    }

    Ok(results_count)
}
