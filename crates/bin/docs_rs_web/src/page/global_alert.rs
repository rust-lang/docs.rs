use anyhow::{Context as _, Result};
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

#[derive(Debug)]
struct State {
    value: Option<GlobalAlert>,
}

#[derive(Debug)]
pub(crate) struct GlobalAlertCache {
    background_task: JoinHandle<()>,
    state: Arc<RwLock<State>>,
}

impl GlobalAlertCache {
    const TTL: Duration = Duration::from_secs(600); // 5 minutes

    pub(crate) async fn new(pool: Pool) -> Self {
        Self::new_with_ttl(pool, Self::TTL).await
    }

    async fn new_with_ttl(pool: Pool, ttl: Duration) -> Self {
        let initial_value = match Self::load_from_pool(&pool).await {
            Ok(value) => value,
            Err(err) => {
                error!(?err, "failed to load initial global alert");
                None
            }
        };

        let state = Arc::new(RwLock::new(State {
            value: initial_value,
        }));
        let refresh_state = Arc::clone(&state);

        let background_task = tokio::spawn(async move {
            let mut refresh_interval = interval(ttl);
            refresh_interval.set_missed_tick_behavior(MissedTickBehavior::Skip);

            // Consume the immediate tick because we already did the initial load.
            refresh_interval.tick().await;

            loop {
                refresh_interval.tick().await;

                debug!("loading global alert from database");
                match Self::load_from_pool(&pool).await {
                    Ok(value) => {
                        let mut state = refresh_state.write().await;
                        state.value = value;
                    }
                    Err(err) => {
                        error!(?err, "failed to refresh global alert, keeping cached value");
                    }
                }
            }
        });

        Self {
            state,
            background_task,
        }
    }

    async fn load_from_pool(pool: &Pool) -> Result<Option<GlobalAlert>> {
        let mut conn = pool
            .get_async()
            .await
            .context("failed to get DB connection for global alert")?;

        get_config::<GlobalAlert>(&mut conn, ConfigName::GlobalAlert)
            .await
            .context("failed to load global alert from config")
    }

    pub(crate) async fn get(&self) -> Option<GlobalAlert> {
        self.state.read().await.value.clone()
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
    use docs_rs_database::service_config::{AlertSeverity, ConfigName, set_config};
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

        let cache =
            GlobalAlertCache::new_with_ttl(env.pool()?.clone(), Duration::from_millis(25)).await;

        assert_eq!(
            cache.get().await.as_ref().map(|alert| alert.text.as_str()),
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
            cache.get().await,
            Some(GlobalAlert {
                url: "https://example.com/maintenance".into(),
                text: "Scheduled maintenance".into(),
                severity: AlertSeverity::Warn,
            })
        );

        Ok(())
    }
}
