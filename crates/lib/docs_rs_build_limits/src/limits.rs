use crate::config::Config;
#[cfg(feature = "database")]
use crate::overrides::Overrides;
#[cfg(feature = "database")]
use docs_rs_types::KrateName;
use serde::Serialize;
use std::time::Duration;

const GB: usize = 1024 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, bon::Builder)]
#[builder(on(_, into, overwritable))]
pub struct Limits {
    #[builder(default = 3 * GB)]
    pub memory: usize,
    #[builder(default = crate::DEFAULT_MAX_TARGETS)]
    pub targets: usize,
    #[builder(default = Duration::from_secs(15 * 60))] // 15 minutes
    pub timeout: Duration,
    #[builder(default = false)]
    pub networking: bool,
    #[builder(default = 100usize * 1024)] // 100 KiB
    pub max_log_size: usize,
}

use limits_builder::State;

impl<S: State> LimitsBuilder<S> {
    pub fn load_config(self, config: &Config) -> LimitsBuilder<S> {
        self.maybe_memory(config.build_default_memory_limit)
    }
}

impl Default for Limits {
    fn default() -> Self {
        Self::builder().build()
    }
}

impl Limits {
    pub fn from_config(config: &Config) -> Limits {
        Self::builder().load_config(config).build()
    }

    #[cfg(feature = "database")]
    pub async fn for_crate(
        config: &Config,
        conn: &mut sqlx::PgConnection,
        name: &KrateName,
    ) -> anyhow::Result<Self> {
        let overrides = Overrides::for_crate(conn, name).await?.unwrap_or_default();

        Ok(Self::builder()
            .load_config(config)
            .maybe_memory(overrides.memory)
            .maybe_targets(overrides.targets)
            .maybe_timeout(overrides.timeout)
            .build())
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
    use docs_rs_config::AppConfig as _;
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
        let mut conn = db.async_conn().await?;

        let cfg = Config::default();

        let defaults = Limits::from_config(&cfg);

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

        let mut conn = db.async_conn().await?;
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

        let mut conn = db.async_conn().await?;

        let limits = Limits::for_crate(&cfg, &mut conn, &KRATE).await?;
        assert_eq!(limits.memory, 6 * GB);

        Ok(())
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn overrides_dont_lower_memory_limit() -> Result<()> {
        let db = db().await?;
        let mut conn = db.async_conn().await?;

        let cfg = Config::default();

        let defaults = Limits::from_config(&cfg);

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
