use anyhow::{Context as _, Result};
use docs_rs_build_queue::AsyncBuildQueue;
use docs_rs_database::{
    Pool,
    service_config::{ConfigName, GlobalAlert, get_config},
};
use std::{sync::Arc, time::Duration};
use tokio::{
    sync::RwLock,
    task::JoinHandle,
    time::{MissedTickBehavior, interval},
};
use tracing::{debug, error};

pub(crate) type ActiveAlerts = Vec<GlobalAlert>;

#[derive(Debug)]
struct State {
    snapshot: ActiveAlerts,
}

#[derive(Debug)]
pub(crate) struct GlobalAlertCache {
    background_task: JoinHandle<()>,
    state: Arc<RwLock<State>>,
}

impl GlobalAlertCache {
    const TTL: Duration = Duration::from_secs(300); // 5 minutes

    pub(crate) async fn new(pool: Pool, build_queue: Arc<AsyncBuildQueue>) -> Self {
        Self::new_with_ttl(pool, build_queue, Self::TTL).await
    }

    async fn new_with_ttl(pool: Pool, build_queue: Arc<AsyncBuildQueue>, ttl: Duration) -> Self {
        let initial_snapshot = match Self::load_from_sources(&pool, &build_queue).await {
            Ok(snapshot) => snapshot,
            Err(err) => {
                error!(?err, "failed to load initial alerts snapshot");
                ActiveAlerts::default()
            }
        };

        let state = Arc::new(RwLock::new(State {
            snapshot: initial_snapshot,
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
                match Self::load_from_sources(&pool, &build_queue).await {
                    Ok(snapshot) => {
                        let mut state = refresh_state.write().await;
                        state.snapshot = snapshot;
                    }
                    Err(err) => {
                        error!(
                            ?err,
                            "failed to refresh alerts snapshot, keeping cached value"
                        );
                    }
                }
            }
        });

        Self {
            state,
            background_task,
        }
    }

    async fn load_from_sources(pool: &Pool, build_queue: &AsyncBuildQueue) -> Result<ActiveAlerts> {
        let mut conn = pool
            .get_async()
            .await
            .context("failed to get DB connection for alerts")?;

        let mut active_alerts = ActiveAlerts::new();

        if let Some(alert) = get_config::<GlobalAlert>(&mut conn, ConfigName::GlobalAlert)
            .await
            .context("failed to load manual global alert from config")?
        {
            active_alerts.push(alert);
        }

        active_alerts.extend(
            build_queue
                .gather_alerts()
                .await
                .context("failed to load build queue alerts")?,
        );

        Ok(active_alerts)
    }

    pub(crate) async fn get(&self) -> ActiveAlerts {
        self.state.read().await.snapshot.clone()
    }
}

impl Drop for GlobalAlertCache {
    fn drop(&mut self) {
        self.background_task.abort();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::TestEnvironment;
    use anyhow::Result;
    use docs_rs_database::service_config::{AlertSeverity, set_config};
    use tokio::time::sleep;

    #[tokio::test(flavor = "multi_thread")]
    async fn cache_loads_immediately_and_keeps_previous_value_on_reload_failure() -> Result<()> {
        let env = TestEnvironment::new().await?;

        let mut conn = env.async_conn().await?;
        set_config(
            &mut conn,
            ConfigName::GlobalAlert,
            GlobalAlert {
                url: "https://example.com/maintenance".into(),
                text: "Scheduled maintenance".into(),
                severity: AlertSeverity::Warn,
            },
        )
        .await?;
        drop(conn);

        let cache = GlobalAlertCache::new_with_ttl(
            env.pool()?.clone(),
            env.build_queue()?.clone(),
            Duration::from_millis(25),
        )
        .await;

        assert_eq!(
            cache.get().await.first().map(|alert| alert.text.as_str()),
            Some("Scheduled maintenance")
        );

        let mut conn = env.async_conn().await?;
        sqlx::query!(
            "UPDATE config SET value = $2 WHERE name = $1",
            "global_alert",
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
            cache.get().await.first(),
            Some(&GlobalAlert {
                url: "https://example.com/maintenance".into(),
                text: "Scheduled maintenance".into(),
                severity: AlertSeverity::Warn,
            })
        );

        Ok(())
    }
}
