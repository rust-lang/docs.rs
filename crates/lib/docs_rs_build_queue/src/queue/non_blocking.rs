use crate::{Config, QueuedCrate, metrics};
use anyhow::Result;
use docs_rs_database::{
    Pool,
    service_config::{ConfigName, get_config, set_config},
};
use docs_rs_opentelemetry::AnyMeterProvider;
use docs_rs_types::{KrateName, Version};
use futures_util::TryStreamExt as _;
use std::{collections::HashMap, sync::Arc};

#[derive(Debug)]
pub struct AsyncBuildQueue {
    pub(super) config: Arc<Config>,
    pub(super) db: Pool,
    pub(super) queue_metrics: metrics::BuildQueueMetrics,
}

impl AsyncBuildQueue {
    pub fn new(db: Pool, config: Arc<Config>, otel_meter_provider: &AnyMeterProvider) -> Self {
        AsyncBuildQueue {
            config,
            db,
            queue_metrics: metrics::BuildQueueMetrics::new(otel_meter_provider),
        }
    }

    pub async fn add_crate(
        &self,
        name: &KrateName,
        version: &Version,
        priority: i32,
        registry: Option<&str>,
    ) -> Result<()> {
        let mut conn = self.db.get_async().await?;

        sqlx::query!(
            "INSERT INTO queue (name, version, priority, registry)
             VALUES ($1, $2, $3, $4)
             ON CONFLICT (name, version) DO UPDATE
                SET priority = EXCLUDED.priority,
                    registry = EXCLUDED.registry,
                    attempt = 0,
                    last_attempt = NULL
            ;",
            name as _,
            version as _,
            priority,
            registry,
        )
        .execute(&mut *conn)
        .await?;

        self.queue_metrics.queued_builds.add(1, &[]);

        Ok(())
    }

    pub async fn pending_count(&self) -> Result<usize> {
        Ok(self
            .pending_count_by_priority()
            .await?
            .values()
            .sum::<usize>())
    }

    pub async fn prioritized_count(&self) -> Result<usize> {
        Ok(self
            .pending_count_by_priority()
            .await?
            .iter()
            .filter(|&(&priority, _)| priority <= 0)
            .map(|(_, count)| count)
            .sum::<usize>())
    }

    pub async fn pending_count_by_priority(&self) -> Result<HashMap<i32, usize>> {
        let mut conn = self.db.get_async().await?;

        Ok(sqlx::query!(
            r#"
            SELECT
                priority,
                COUNT(*) as "count!"
            FROM queue
            GROUP BY priority"#,
        )
        .fetch(&mut *conn)
        .map_ok(|row| (row.priority, row.count as usize))
        .try_collect()
        .await?)
    }

    pub async fn queued_crates(&self) -> Result<Vec<QueuedCrate>> {
        let mut conn = self.db.get_async().await?;

        Ok(sqlx::query_as!(
            QueuedCrate,
            r#"SELECT
                id,
                name as "name: KrateName",
                version as "version: Version",
                priority,
                registry,
                attempt
             FROM queue
             ORDER BY priority ASC, attempt ASC, id ASC"#,
        )
        .fetch_all(&mut *conn)
        .await?)
    }

    pub async fn has_build_queued(&self, name: &KrateName, version: &Version) -> Result<bool> {
        let mut conn = self.db.get_async().await?;
        Ok(sqlx::query_scalar!(
            "SELECT id
             FROM queue
             WHERE
                name = $1 AND
                version = $2
             ",
            name as _,
            version as _,
        )
        .fetch_optional(&mut *conn)
        .await?
        .is_some())
    }

    pub async fn remove_crate_from_queue(&self, name: &KrateName) -> Result<()> {
        let mut conn = self.db.get_async().await?;
        sqlx::query!(
            "DELETE
             FROM queue
             WHERE name = $1
             ",
            name as _
        )
        .execute(&mut *conn)
        .await?;

        Ok(())
    }

    pub async fn remove_version_from_queue(
        &self,
        name: &KrateName,
        version: &Version,
    ) -> Result<()> {
        let mut conn = self.db.get_async().await?;
        sqlx::query!(
            "DELETE
             FROM queue
             WHERE
                name = $1 AND
                version = $2
             ",
            name as _,
            version as _,
        )
        .execute(&mut *conn)
        .await?;

        Ok(())
    }

    /// Decreases the priority of all releases currently present in the queue not matching the version passed to *at least* new_priority.
    pub async fn deprioritize_other_releases(
        &self,
        name: &KrateName,
        latest_version: &Version,
        new_priority: i32,
    ) -> Result<()> {
        let mut conn = self.db.get_async().await?;
        sqlx::query!(
            "UPDATE queue
             SET priority = GREATEST(priority, $1)
             WHERE
                name = $2
                AND version != $3
             ",
            new_priority,
            name as _,
            latest_version as _,
        )
        .execute(&mut *conn)
        .await?;

        Ok(())
    }
}

/// Locking functions.
impl AsyncBuildQueue {
    /// Checks for the lock and returns whether it currently exists.
    pub async fn is_locked(&self) -> Result<bool> {
        let mut conn = self.db.get_async().await?;

        Ok(get_config::<bool>(&mut conn, ConfigName::QueueLocked)
            .await?
            .unwrap_or(false))
    }

    /// lock the queue. Daemon will check this lock and stop operating if it exists.
    pub async fn lock(&self) -> Result<()> {
        let mut conn = self.db.get_async().await?;
        set_config(&mut conn, ConfigName::QueueLocked, true).await
    }

    /// unlock the queue.
    pub async fn unlock(&self) -> Result<()> {
        let mut conn = self.db.get_async().await?;
        set_config(&mut conn, ConfigName::QueueLocked, false).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use docs_rs_config::AppConfig as _;
    use docs_rs_database::testing::TestDatabase;
    use docs_rs_opentelemetry::testing::TestMetrics;
    use docs_rs_types::testing::{KRATE, V1, V2};
    use pretty_assertions::assert_eq;

    const FAILED_KRATE: KrateName = KrateName::from_static("failed_crate");

    // when we start migrating / spitting the binaries,
    // we probably will create  amore powerfull & flexible
    // test& app context. Then we could migrate this.
    struct TestEnv {
        db: TestDatabase,
        queue: AsyncBuildQueue,
    }

    async fn test_queue() -> Result<TestEnv> {
        let test_metrics = TestMetrics::new();
        let db = TestDatabase::new(
            &docs_rs_database::Config::test_config()?,
            test_metrics.provider(),
        )
        .await?;

        let queue = AsyncBuildQueue::new(
            db.pool().clone(),
            Arc::new(Config::from_environment()?),
            test_metrics.provider(),
        );

        Ok(TestEnv { db, queue })
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_add_duplicate_doesnt_fail_last_priority_wins() -> Result<()> {
        let env = test_queue().await?;
        let queue = env.queue;

        queue.add_crate(&KRATE, &V1, 0, None).await?;
        queue.add_crate(&KRATE, &V1, 9, None).await?;

        let queued_crates = queue.queued_crates().await?;
        assert_eq!(queued_crates.len(), 1);
        assert_eq!(queued_crates[0].priority, 9);

        Ok(())
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_add_duplicate_resets_attempts_and_priority() -> Result<()> {
        let env = test_queue().await?;
        let queue = env.queue;

        assert_eq!(queue.pending_count().await?, 0);

        let mut conn = env.db.async_conn().await?;
        sqlx::query!(
            "INSERT INTO queue (name, version, priority, attempt, last_attempt )
             VALUES ($1, $2, 0, 5, NOW())",
            FAILED_KRATE as _,
            V1 as _
        )
        .execute(&mut *conn)
        .await?;

        assert_eq!(queue.pending_count().await?, 1);

        queue.add_crate(&FAILED_KRATE, &V1, 9, None).await?;

        assert_eq!(queue.pending_count().await?, 1);

        let row = sqlx::query!(
            "SELECT priority, attempt, last_attempt
             FROM queue
             WHERE name = $1 AND version = $2",
            FAILED_KRATE as _,
            V1 as _
        )
        .fetch_one(&mut *conn)
        .await?;

        assert_eq!(row.priority, 9);
        assert_eq!(row.attempt, 0);
        assert!(row.last_attempt.is_none());
        Ok(())
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_has_build_queued() -> Result<()> {
        let env = test_queue().await?;
        let queue = env.queue;

        queue.add_crate(&KRATE, &V1, 0, None).await?;

        let mut conn = env.db.async_conn().await?;
        assert!(queue.has_build_queued(&KRATE, &V1).await.unwrap());

        sqlx::query!("DELETE FROM queue")
            .execute(&mut *conn)
            .await
            .unwrap();

        assert!(!queue.has_build_queued(&KRATE, &V1).await.unwrap());

        Ok(())
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_delete_version_from_queue() -> Result<()> {
        let env = test_queue().await?;
        let queue = env.queue;

        assert_eq!(queue.pending_count().await?, 0);

        queue.add_crate(&KRATE, &V1, 0, None).await?;
        queue.add_crate(&KRATE, &V2, 0, None).await?;

        assert_eq!(queue.pending_count().await?, 2);
        queue.remove_version_from_queue(&KRATE, &V1).await?;

        assert_eq!(queue.pending_count().await?, 1);

        // only v2 remains
        if let [k] = queue.queued_crates().await?.as_slice() {
            assert_eq!(k.name, KRATE);
            assert_eq!(k.version, V2);
        } else {
            panic!("expected only one queued crate");
        }

        Ok(())
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_delete_crate_from_queue() -> Result<()> {
        let env = test_queue().await?;
        let queue = env.queue;

        assert_eq!(queue.pending_count().await?, 0);

        queue.add_crate(&KRATE, &V1, 0, None).await?;
        queue.add_crate(&KRATE, &V2, 0, None).await?;

        assert_eq!(queue.pending_count().await?, 2);
        queue.remove_crate_from_queue(&KRATE).await?;

        assert_eq!(queue.pending_count().await?, 0);

        Ok(())
    }
}
