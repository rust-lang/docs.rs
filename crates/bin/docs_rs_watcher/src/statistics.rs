use anyhow::Result;
use chrono::{Duration, Utc};
use docs_rs_database::service_config::{
    ConfigName, DocumentationStatistics, get_config, set_config,
};
use sqlx::Connection as _;
use tracing::{info, instrument};

/// Refresh the cached documentation file count at most once every twelve hours.
#[instrument(skip_all)]
pub async fn gather_documentation_statistics(conn: &mut sqlx::PgConnection) -> Result<()> {
    let cached: Option<DocumentationStatistics> =
        get_config(conn, ConfigName::DocumentationStatistics).await?;

    if cached.is_some_and(|stats| Utc::now() - stats.last_updated() < Duration::hours(12)) {
        return Ok(());
    }

    info!("updating cached documentation statistics");

    let mut tx = conn.begin().await?;

    // set a statement timeout, so this expensive query won't run wild.
    sqlx::query!("SET LOCAL statement_timeout = '15s'")
        .execute(&mut *tx)
        .await?;
    sqlx::query!("SET LOCAL lock_timeout = '1s'")
        .execute(&mut *tx)
        .await?;

    // tested on prod, on 2026-10-30, with ~3 mio releases:
    // ~5 seconds
    //
    // more optimizations possible, we'll try again on the new RDS server.
    let count = sqlx::query_scalar!(
        r#"SELECT COALESCE(SUM(b.documentation_file_count), 0)::BIGINT AS "count!"
           FROM releases r
           JOIN (
               SELECT DISTINCT ON (rid) rid, documentation_file_count
               FROM builds
               WHERE build_status = 'success'
               ORDER BY rid, id DESC
           ) b ON b.rid = r.id
           WHERE r.rustdoc_status = TRUE"#,
    )
    .fetch_one(&mut *tx)
    .await?;
    tx.commit().await?;

    set_config(
        conn,
        ConfigName::DocumentationStatistics,
        DocumentationStatistics::new(count.try_into()?),
    )
    .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::TestEnvironment;
    use docs_rs_test_fakes::FakeBuild;
    use docs_rs_types::testing::{BAR, BAZ, FOO, KRATE};
    use test_case::test_case;

    #[tokio::test(flavor = "multi_thread")]
    async fn counts_latest_successful_build_per_documented_release() -> Result<()> {
        let env = TestEnvironment::builder().build().await?;
        env.fake_release()
            .await
            .name(&FOO)
            .builds(vec![
                // older successful build,  ignored
                FakeBuild::default().documentation_file_count(Some(100)),
                // last successful build, count will be used
                FakeBuild::default().documentation_file_count(Some(7)),
                // latest build = failed, count will be ignored (but shouldn't exist anyways)
                FakeBuild::default()
                    .successful(false)
                    .documentation_file_count(Some(999)),
            ])
            .create()
            .await?;
        let mut conn = env.async_conn().await?;

        // failed release, will be ignored
        env.fake_release()
            .await
            .name(&BAZ)
            .build_result_failed()
            .create()
            .await?;

        for (name, count) in [
            // release/build with count, will be used
            (&BAR, Some(13_u64)),
            // release/build without count, will be ignored
            (&KRATE, None),
        ] {
            env.fake_release()
                .await
                .name(name)
                .builds(vec![FakeBuild::default().documentation_file_count(count)])
                .create()
                .await?;
        }

        gather_documentation_statistics(&mut conn).await?;

        let stats: DocumentationStatistics =
            get_config(&mut conn, ConfigName::DocumentationStatistics)
                .await?
                .unwrap();
        assert_eq!(stats.documentation_file_count(), 20);
        Ok(())
    }

    #[test_case(None, 0; "initializes_empty_database")]
    #[test_case(Some(1), 42; "preserves_fresh_cache")]
    #[test_case(Some(12), 0; "refreshes_stale_cache")]
    #[tokio::test(flavor = "multi_thread")]
    async fn refreshes_only_when_needed(age_hours: Option<i64>, expected: u64) -> Result<()> {
        let env = TestEnvironment::builder().build().await?;
        let mut conn = env.async_conn().await?;
        let cached = age_hours
            .map(|hours| DocumentationStatistics::new_at(42, Utc::now() - Duration::hours(hours)));

        if let Some(cached) = &cached {
            set_config(&mut conn, ConfigName::DocumentationStatistics, cached).await?;
        }

        let before = Utc::now();
        gather_documentation_statistics(&mut conn).await?;

        let stats: DocumentationStatistics =
            get_config(&mut conn, ConfigName::DocumentationStatistics)
                .await?
                .unwrap();
        assert_eq!(stats.documentation_file_count(), expected);
        if expected == 42 {
            assert_eq!(Some(stats), cached);
        } else {
            assert!(stats.last_updated() >= before);
        }
        Ok(())
    }
}
