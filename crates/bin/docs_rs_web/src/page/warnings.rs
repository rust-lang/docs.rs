use anyhow::{Context as _, Result};
use chrono::Utc;
use docs_rs_build_queue::AsyncBuildQueue;
use docs_rs_database::{
    Pool,
    service_config::{Abnormality, ConfigName, get_config},
};
use serde::Serialize;
use std::{sync::Arc, time::Duration};
use tokio::{
    sync::RwLock,
    task::JoinHandle,
    time::{MissedTickBehavior, interval},
};
use tracing::{debug, error};

pub(crate) type ActiveAbnormalities = Vec<Abnormality>;

#[derive(Debug, Clone, Serialize)]
pub(crate) struct ActiveWarnings {
    pub(crate) abnormalities: ActiveAbnormalities,
}

/// cache for warning items to be shown on mane pages.
/// * abnormalities (long build queue, cpu usage / response times, ...)
/// * later alerts / notifications (to be discarded by the user)
#[derive(Debug)]
pub(crate) struct WarningsCache {
    background_task: JoinHandle<()>,
    state: Arc<RwLock<ActiveWarnings>>,
}

impl WarningsCache {
    const TTL: Duration = Duration::from_secs(300); // 5 minutes

    pub(crate) async fn new(pool: Pool, build_queue: Arc<AsyncBuildQueue>) -> Self {
        Self::new_with_ttl(pool, build_queue, Self::TTL).await
    }

    async fn new_with_ttl(pool: Pool, build_queue: Arc<AsyncBuildQueue>, ttl: Duration) -> Self {
        async fn load_abnormalities(
            pool: &Pool,
            build_queue: &AsyncBuildQueue,
            previous_snapshot: &[Abnormality],
        ) -> Option<ActiveAbnormalities> {
            match WarningsCache::load_abnormalities(pool, build_queue, previous_snapshot).await {
                Ok(snapshot) => Some(snapshot),
                Err(err) => {
                    error!(?err, "failed to load abnormalities");
                    None
                }
            }
        }

        let initial_abnormalities = load_abnormalities(&pool, &build_queue, &[])
            .await
            .unwrap_or_default();

        let state = Arc::new(RwLock::new(ActiveWarnings {
            abnormalities: initial_abnormalities,
        }));
        let refresh_state = Arc::clone(&state);

        let background_task = tokio::spawn(async move {
            let mut refresh_interval = interval(ttl);
            refresh_interval.set_missed_tick_behavior(MissedTickBehavior::Skip);

            // Consume the immediate tick because we already did the initial load.
            refresh_interval.tick().await;

            loop {
                refresh_interval.tick().await;

                debug!("loading alerts snapshot");
                let previous_abnormalities = refresh_state.read().await.abnormalities.clone();

                if let Some(abnormalities) =
                    load_abnormalities(&pool, &build_queue, &previous_abnormalities).await
                {
                    let mut state = refresh_state.write().await;
                    state.abnormalities = abnormalities;
                }
            }
        });

        Self {
            state,
            background_task,
        }
    }

    async fn load_abnormalities(
        pool: &Pool,
        build_queue: &AsyncBuildQueue,
        previous_abnormalities: &[Abnormality],
    ) -> Result<ActiveAbnormalities> {
        let mut conn = pool
            .get_async()
            .await
            .context("failed to get DB connection for alerts")?;

        let mut active_abnormalities = ActiveAbnormalities::new();

        if let Some(abnormality) = get_config::<Abnormality>(&mut conn, ConfigName::Abnormality)
            .await
            .context("failed to load manual abnormality from config")?
        {
            active_abnormalities.push(abnormality);
        }

        let mut queue_abnormalities = build_queue
            .gather_alerts()
            .await
            .context("failed to load build queue abnormalities")?;
        for abnormality in &mut queue_abnormalities {
            Self::assign_start_time(abnormality, previous_abnormalities);
        }
        active_abnormalities.extend(queue_abnormalities);

        Ok(active_abnormalities)
    }

    fn same_abnormality(left: &Abnormality, right: &Abnormality) -> bool {
        left.anchor_id == right.anchor_id
    }

    fn assign_start_time(abnormality: &mut Abnormality, previous_snapshot: &[Abnormality]) {
        if abnormality.start_time.is_some() {
            return;
        }

        abnormality.start_time = previous_snapshot
            .iter()
            .find(|previous| Self::same_abnormality(previous, abnormality))
            .and_then(|previous| previous.start_time)
            .or_else(|| Some(Utc::now()));
    }

    pub(crate) async fn get(&self) -> ActiveWarnings {
        self.state.read().await.clone()
    }
}

impl Drop for WarningsCache {
    fn drop(&mut self) {
        self.background_task.abort();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::TestEnvironment;
    use anyhow::Result;
    use docs_rs_config::AppConfig as _;
    use docs_rs_database::service_config::{AlertSeverity, AnchorId, set_config};
    use docs_rs_uri::EscapedURI;
    use tokio::time::sleep;

    #[tokio::test(flavor = "multi_thread")]
    async fn cache_loads_immediately_and_keeps_previous_value_on_reload_failure() -> Result<()> {
        let env = TestEnvironment::new().await?;

        let mut conn = env.async_conn().await?;
        set_config(
            &mut conn,
            ConfigName::Abnormality,
            Abnormality {
                anchor_id: AnchorId::Manual,
                url: "https://example.com/maintenance"
                    .parse::<EscapedURI>()
                    .unwrap(),
                text: "Scheduled maintenance".into(),
                explanation: Some("Planned maintenance is in progress.".into()),
                start_time: None,
                severity: AlertSeverity::Warn,
            },
        )
        .await?;
        drop(conn);

        let cache = WarningsCache::new_with_ttl(
            env.pool()?.clone(),
            env.build_queue()?.clone(),
            Duration::from_millis(25),
        )
        .await;

        assert_eq!(
            cache
                .get()
                .await
                .abnormalities
                .first()
                .map(|alert| alert.text.as_str()),
            Some("Scheduled maintenance")
        );

        let mut conn = env.async_conn().await?;
        sqlx::query!(
            "UPDATE config SET value = $2 WHERE name = $1",
            "abnormality",
            serde_json::json!({
                "url": 1,
                "text": false
            }),
        )
        .execute(&mut *conn)
        .await?;
        drop(conn);

        sleep(Duration::from_millis(75)).await;

        assert_eq!(
            cache
                .get()
                .await
                .abnormalities
                .first()
                .map(|abnormality| abnormality.text.as_str()),
            Some("Scheduled maintenance")
        );
        assert_eq!(
            cache
                .get()
                .await
                .abnormalities
                .first()
                .map(|abnormality| &abnormality.anchor_id),
            Some(&AnchorId::Manual)
        );
        assert_eq!(
            cache
                .get()
                .await
                .abnormalities
                .first()
                .and_then(|abnormality| abnormality.start_time),
            None
        );

        Ok(())
    }

    #[test]
    fn same_abnormality_uses_anchor_id_only() {
        let left = Abnormality {
            anchor_id: AnchorId::Manual,
            url: "https://example.com/one".parse::<EscapedURI>().unwrap(),
            text: "first text".into(),
            explanation: Some("first explanation".into()),
            start_time: None,
            severity: AlertSeverity::Warn,
        };
        let right = Abnormality {
            anchor_id: AnchorId::Manual,
            url: "https://example.com/two".parse::<EscapedURI>().unwrap(),
            text: "second text".into(),
            explanation: None,
            start_time: None,
            severity: AlertSeverity::Error,
        };

        assert!(WarningsCache::same_abnormality(&left, &right));
    }

    #[test]
    fn same_abnormality_returns_false_for_different_anchor_ids() {
        let left = Abnormality {
            anchor_id: AnchorId::Manual,
            url: "https://example.com".parse::<EscapedURI>().unwrap(),
            text: "same text".into(),
            explanation: Some("same explanation".into()),
            start_time: None,
            severity: AlertSeverity::Warn,
        };
        let right = Abnormality {
            anchor_id: AnchorId::QueueLength,
            url: "https://example.com".parse::<EscapedURI>().unwrap(),
            text: "same text".into(),
            explanation: Some("same explanation".into()),
            start_time: None,
            severity: AlertSeverity::Warn,
        };

        assert!(!WarningsCache::same_abnormality(&left, &right));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn cache_preserves_queue_start_time_across_refreshes() -> Result<()> {
        let mut queue_config = docs_rs_build_queue::Config::test_config()?;
        queue_config.length_warning_threshold = 1;
        let env = crate::testing::TestEnvironment::builder()
            .build_queue_config(queue_config)
            .build()
            .await?;

        let queue = env.build_queue()?.clone();
        for idx in 0..2 {
            let name = format!("queued-crate-{idx}").parse::<docs_rs_types::KrateName>()?;
            queue
                .add_crate(&name, &docs_rs_types::Version::parse("1.0.0")?, 0, None)
                .await?;
        }

        let cache = WarningsCache::new_with_ttl(
            env.pool()?.clone(),
            env.build_queue()?.clone(),
            Duration::from_millis(25),
        )
        .await;

        let first_snapshot = cache.get().await;
        let first_start_time = first_snapshot
            .abnormalities
            .iter()
            .find(|a| a.anchor_id == AnchorId::QueueLength)
            .expect("missing queue-length abnormality on first load")
            .start_time
            .expect("queue-length abnormality should have a start_time");

        // Wait for at least one cache refresh cycle.
        sleep(Duration::from_millis(75)).await;

        let second_snapshot = cache.get().await;
        let second_start_time = second_snapshot
            .abnormalities
            .iter()
            .find(|a| a.anchor_id == AnchorId::QueueLength)
            .expect("missing queue-length abnormality after refresh")
            .start_time
            .expect("queue-length abnormality should still have a start_time");

        assert_eq!(
            first_start_time, second_start_time,
            "start_time should be preserved across cache refreshes"
        );

        Ok(())
    }
}
