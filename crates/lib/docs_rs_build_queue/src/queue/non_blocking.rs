use crate::{
    Config, PRIORITY_MANUAL_FROM_CRATES_IO, QueuedCrate, metrics, priority::PrioritiesCache,
};
use anyhow::{Context as _, Result};
use docs_rs_database::{
    Pool,
    service_config::{Abnormality, ConfigName, get_config, set_config},
};
use docs_rs_opentelemetry::AnyMeterProvider;
use docs_rs_types::{KrateName, Version};
use docs_rs_uri::EscapedURI;
use futures_util::TryStreamExt as _;
use std::{collections::HashMap, sync::Arc};

#[derive(Debug)]
pub struct AsyncBuildQueue {
    pub(super) config: Arc<Config>,
    pub(super) db: Pool,
    pub(super) queue_metrics: metrics::BuildQueueMetrics,
    pub(super) priorities_cache: PrioritiesCache,
}

impl AsyncBuildQueue {
    pub fn new(db: Pool, config: Arc<Config>, otel_meter_provider: &AnyMeterProvider) -> Self {
        AsyncBuildQueue {
            priorities_cache: PrioritiesCache::new(db.clone(), config.deprioritize_workspace_size),
            config,
            db,
            queue_metrics: metrics::BuildQueueMetrics::new(otel_meter_provider),
        }
    }

    pub async fn find_priority(&self, name: &KrateName) -> Result<i32> {
        self.priorities_cache.get(name).await
    }

    /// Refresh priorities for queued releases that still have the default priority.
    ///
    /// Queue-specific priorities, such as manual rebuilds and older releases, are
    /// deliberately left unchanged.
    pub async fn refresh_default_priorities(&self) -> Result<()> {
        let mut conn = self.db.get_async().await?;
        let names = sqlx::query_scalar!(
            r#"
            SELECT DISTINCT name AS "name: KrateName"
            FROM queue
            WHERE priority = $1
            "#,
            crate::PRIORITY_DEFAULT,
        )
        .fetch_all(&mut *conn)
        .await?;

        let mut changed_names = Vec::new();
        let mut changed_priorities = Vec::new();

        for name in names {
            let priority = self.find_priority(&name).await?;
            if priority != crate::PRIORITY_DEFAULT {
                changed_names.push(name.to_string());
                changed_priorities.push(priority);
            }
        }

        if changed_names.is_empty() || changed_priorities.is_empty() {
            return Ok(());
        }

        sqlx::query!(
            r#"
            UPDATE queue
            SET priority = updates.priority
            FROM UNNEST($1::text[], $2::int[]) AS updates(name, priority)
            WHERE
                queue.name = updates.name
                AND queue.priority = $3
            "#,
            &changed_names,
            &changed_priorities,
            crate::PRIORITY_DEFAULT,
        )
        .execute(&mut *conn)
        .await?;

        Ok(())
    }

    pub async fn add_crate(
        &self,
        name: &KrateName,
        version: &Version,
        priority: i32,
    ) -> Result<()> {
        let mut conn = self.db.get_async().await?;

        sqlx::query!(
            "INSERT INTO queue (name, version, priority)
             VALUES ($1, $2, $3)
             ON CONFLICT (name, version) DO UPDATE
                SET priority = EXCLUDED.priority,
                    attempt = 0,
                    last_attempt = NULL
            ;",
            name as _,
            version as _,
            priority,
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

    pub fn build_queue_is_too_long<'a>(
        &self,
        queued_crates: impl Iterator<Item = &'a QueuedCrate>,
    ) -> bool {
        queued_crates
            .filter(|qc| qc.priority < PRIORITY_MANUAL_FROM_CRATES_IO)
            .count()
            > self.config.length_warning_threshold
    }

    /// fetch the current queue alerts
    pub async fn gather_alerts(&self) -> Result<Vec<Abnormality>> {
        let queue_pending_count = self
            .pending_count_by_priority()
            .await
            .context("failed to fetch queue length for alerts")?
            .into_iter()
            .filter_map(|(prio, amount)| (prio < PRIORITY_MANUAL_FROM_CRATES_IO).then_some(amount))
            .sum::<usize>();

        let mut alerts = Vec::with_capacity(1);

        if queue_pending_count > self.config.length_warning_threshold {
            alerts.push(Abnormality {
                url: EscapedURI::from_path("/releases/queue"),
                text: "long build queue".into(),
                explanation: Some(
                    format!(
                        "The build queue currently contains more than {} crates, so it might take a while before new published crates get documented.",
                        self.config.length_warning_threshold,
                    )
                ),
            });
        }

        Ok(alerts)
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
    use docs_rs_repository_stats::workspaces::{
        rewrite_repository_stats, set_repository_build_priority,
    };
    use docs_rs_storage::testing::TestStorage;
    use docs_rs_test_fakes::{FakeGithubStats, FakeRelease};
    use docs_rs_types::testing::{BAR, BAZ, FOO, KRATE, V1, V2};
    use pretty_assertions::assert_eq;

    const FAILED_KRATE: KrateName = KrateName::from_static("failed_crate");
    const REPO: &str = "owner1/repo1";

    // when we start migrating / spitting the binaries,
    // we probably will create  amore powerfull & flexible
    // test& app context. Then we could migrate this.
    struct TestEnv {
        db: TestDatabase,
        storage: TestStorage,
        queue: AsyncBuildQueue,
    }

    impl TestEnv {
        async fn fake_release(&self) -> FakeRelease<'_> {
            FakeRelease::new(self.db.pool().clone(), self.storage.storage().clone())
        }
    }

    async fn test_queue() -> Result<TestEnv> {
        test_queue_with_config(Config::from_environment()?).await
    }

    async fn test_queue_with_config(config: Config) -> Result<TestEnv> {
        let test_metrics = TestMetrics::new();
        let db = TestDatabase::new(
            &docs_rs_database::Config::test_config()?,
            test_metrics.provider(),
        )
        .await?;

        let storage = TestStorage::from_config(
            docs_rs_storage::Config::test_config()?.into(),
            test_metrics.provider(),
        )
        .await?;

        let queue =
            AsyncBuildQueue::new(db.pool().clone(), Arc::new(config), test_metrics.provider());

        Ok(TestEnv { db, storage, queue })
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_add_duplicate_doesnt_fail_last_priority_wins() -> Result<()> {
        let env = test_queue().await?;
        let queue = env.queue;

        queue.add_crate(&KRATE, &V1, 0).await?;
        queue.add_crate(&KRATE, &V1, 9).await?;

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

        queue.add_crate(&FAILED_KRATE, &V1, 9).await?;

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
    async fn test_refresh_default_priorities_deprioritizes_queued_workspace() -> Result<()> {
        let mut config = Config::from_environment()?;
        config.deprioritize_workspace_size = 1;
        let env = test_queue_with_config(config).await?;
        let mut conn = env.db.async_conn().await?;

        let repo_id = FakeGithubStats::builder()
            .repo(REPO)
            .create(&mut conn)
            .await?;

        for name in [FOO, BAR] {
            env.fake_release()
                .await
                .name(&name)
                .version(V1)
                .github_stats_id(repo_id)
                .create()
                .await?;
        }

        for name in [FOO, BAR, BAZ] {
            env.queue
                .add_crate(&name, &V1, crate::PRIORITY_DEFAULT)
                .await?;
        }

        rewrite_repository_stats(&mut conn).await?;
        env.queue.priorities_cache.reload().await?;
        env.queue.refresh_default_priorities().await?;

        assert_eq!(
            env.queue
                .queued_crates()
                .await?
                .into_iter()
                .map(|queued| (queued.name, queued.priority))
                .collect::<Vec<_>>(),
            vec![
                (BAZ, crate::PRIORITY_DEFAULT),
                (FOO, crate::PRIORITY_DEPRIORITIZED),
                (BAR, crate::PRIORITY_DEPRIORITIZED),
            ]
        );

        Ok(())
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_refresh_default_priorities_applies_repository_override() -> Result<()> {
        let env = test_queue().await?;
        let mut conn = env.db.async_conn().await?;

        env.fake_release()
            .await
            .name(&FOO)
            .version(V1)
            .github_stats(REPO, 0, 0, 0)
            .create()
            .await?;

        env.queue
            .add_crate(&FOO, &V1, crate::PRIORITY_DEFAULT)
            .await?;
        env.queue
            .add_crate(&BAR, &V1, crate::PRIORITY_MANUAL_FROM_CRATES_IO)
            .await?;

        set_repository_build_priority(&mut conn, REPO, -10).await?;
        env.queue.priorities_cache.reload().await?;
        env.queue.refresh_default_priorities().await?;

        assert_eq!(
            env.queue
                .queued_crates()
                .await?
                .into_iter()
                .map(|queued| (queued.name, queued.priority))
                .collect::<Vec<_>>(),
            vec![(FOO, -10), (BAR, crate::PRIORITY_MANUAL_FROM_CRATES_IO),]
        );

        Ok(())
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_has_build_queued() -> Result<()> {
        let env = test_queue().await?;
        let queue = env.queue;

        queue.add_crate(&KRATE, &V1, 0).await?;

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

        queue.add_crate(&KRATE, &V1, 0).await?;
        queue.add_crate(&KRATE, &V2, 0).await?;

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

        queue.add_crate(&KRATE, &V1, 0).await?;
        queue.add_crate(&KRATE, &V2, 0).await?;

        assert_eq!(queue.pending_count().await?, 2);
        queue.remove_crate_from_queue(&KRATE).await?;

        assert_eq!(queue.pending_count().await?, 0);

        Ok(())
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_length_warning_threshold_boundary() -> Result<()> {
        let mut config = Config::from_environment()?;
        config.length_warning_threshold = 1;
        let env = test_queue_with_config(config).await?;
        let queue = env.queue;

        queue.add_crate(&FOO, &V1, 0).await?;

        assert!(!queue.build_queue_is_too_long(queue.queued_crates().await?.iter()));
        assert!(queue.gather_alerts().await?.is_empty());

        queue.add_crate(&BAR, &V1, 0).await?;

        assert!(queue.build_queue_is_too_long(queue.queued_crates().await?.iter()));
        assert_eq!(
            queue.gather_alerts().await?,
            vec![Abnormality {
                url: EscapedURI::from_path("/releases/queue"),
                text: "long build queue".into(),
                explanation: Some("The build queue currently contains more than 1 crates, so it might take a while before new published crates get documented.".into()),
            }]
        );

        Ok(())
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_public_alert_ignores_manual_crates() -> Result<()> {
        let mut config = Config::from_environment()?;
        config.length_warning_threshold = 0;
        let env = test_queue_with_config(config).await?;
        let queue = env.queue;

        queue
            .add_crate(&FOO, &V1, PRIORITY_MANUAL_FROM_CRATES_IO)
            .await?;

        assert!(!queue.build_queue_is_too_long(queue.queued_crates().await?.iter()));
        assert!(queue.gather_alerts().await?.is_empty());

        Ok(())
    }
}
