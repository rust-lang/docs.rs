use crate::{config::Config, overrides::Overrides};
use anyhow::Result;
use docs_rs_types::KrateName;
use serde::Serialize;
use std::time::Duration;

const GB: usize = 1024 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Limits {
    pub memory: usize,
    pub targets: usize,
    pub timeout: Duration,
    pub networking: bool,
    pub max_log_size: usize,
}

impl Limits {
    pub fn new(config: &Config) -> Self {
        Self {
            // 3 GB default default
            memory: config.build_default_memory_limit.unwrap_or(3 * GB),
            timeout: Duration::from_secs(15 * 60), // 15 minutes
            targets: crate::DEFAULT_MAX_TARGETS,
            networking: false,
            max_log_size: 100 * 1024, // 100 KB
        }
    }

    pub async fn for_crate(
        config: &Config,
        conn: &mut sqlx::PgConnection,
        name: &KrateName,
    ) -> Result<Self> {
        let default = Self::new(config);
        let overrides = Overrides::for_crate(conn, name).await?.unwrap_or_default();
        Ok(Self {
            memory: overrides
                .memory
                .unwrap_or(default.memory)
                .max(default.memory),
            targets: overrides
                .targets
                .or(overrides.timeout.map(|_| 1))
                .unwrap_or(default.targets),
            timeout: overrides.timeout.unwrap_or(default.timeout),
            networking: default.networking,
            max_log_size: default.max_log_size,
        })
    }

    pub fn memory(&self) -> usize {
        self.memory
    }

    pub fn timeout(&self) -> Duration {
        self.timeout
    }

    pub fn networking(&self) -> bool {
        self.networking
    }

    pub fn max_log_size(&self) -> usize {
        self.max_log_size
    }

    pub fn targets(&self) -> usize {
        self.targets
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use docs_rs_database::testing::TestDatabase;
    use docs_rs_opentelemetry::testing::TestMetrics;
    use docs_rs_types::testing::KRATE;

    async fn db() -> anyhow::Result<TestDatabase> {
        let test_metrics = TestMetrics::new();
        TestDatabase::new(
            &docs_rs_database::Config::test_config()?,
            test_metrics.provider(),
        )
        .await
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn retrieve_limits() -> anyhow::Result<()> {
        let db = db().await?;
        let mut conn = db.async_conn().await;

        let cfg = Config::default();

        let defaults = Limits::new(&cfg);

        let krate = KrateName::from_static("hexponent");
        // limits work if no crate has limits set
        let hexponent = Limits::for_crate(&cfg, &mut conn, &krate).await?;
        assert_eq!(hexponent, defaults);

        Overrides::save(
            &mut conn,
            &krate,
            Overrides {
                targets: Some(15),
                ..Overrides::default()
            },
        )
        .await?;
        // limits work if crate has limits set
        let hexponent = Limits::for_crate(&cfg, &mut conn, &krate).await?;
        assert_eq!(
            hexponent,
            Limits {
                targets: 15,
                ..defaults
            }
        );

        // all limits work
        let krate = KrateName::from_static("regex");
        let limits = Limits {
            memory: defaults.memory * 2,
            timeout: defaults.timeout * 2,
            targets: 1,
            ..defaults
        };
        Overrides::save(
            &mut conn,
            &krate,
            Overrides {
                memory: Some(limits.memory),
                targets: Some(limits.targets),
                timeout: Some(limits.timeout),
            },
        )
        .await?;
        assert_eq!(limits, Limits::for_crate(&cfg, &mut conn, &krate).await?);
        Ok(())
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn targets_default_to_one_with_timeout() -> anyhow::Result<()> {
        let db = db().await?;

        let mut conn = db.async_conn().await;
        let krate = KrateName::from_static("hexponent");
        Overrides::save(
            &mut conn,
            &krate,
            Overrides {
                timeout: Some(Duration::from_secs(20 * 60)),
                ..Overrides::default()
            },
        )
        .await?;
        let limits = Limits::for_crate(&Config::default(), &mut conn, &krate).await?;
        assert_eq!(limits.targets, 1);

        Ok(())
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn config_default_memory_limit() -> Result<()> {
        let db = db().await?;

        let cfg = Config {
            build_default_memory_limit: Some(6 * GB),
        };

        let mut conn = db.async_conn().await;

        let limits = Limits::for_crate(&cfg, &mut conn, &KRATE).await?;
        assert_eq!(limits.memory, 6 * GB);

        Ok(())
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn overrides_dont_lower_memory_limit() -> Result<()> {
        let db = db().await?;
        let mut conn = db.async_conn().await;

        let cfg = Config::default();

        let defaults = Limits::new(&cfg);

        Overrides::save(
            &mut conn,
            &KRATE,
            Overrides {
                memory: Some(defaults.memory / 2),
                ..Overrides::default()
            },
        )
        .await?;

        let limits = Limits::for_crate(&cfg, &mut conn, &KRATE).await?;
        assert_eq!(limits, defaults);

        Ok(())
    }
}
