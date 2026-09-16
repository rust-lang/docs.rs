use crate::{config::Config, overrides::Overrides};
use anyhow::Result;
use docs_rs_types::{ByteSize, Duration, KrateName};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Limits {
    pub memory: ByteSize,
    pub targets: usize,
    pub timeout: Duration,
    pub networking: bool,
    pub max_log_size: ByteSize,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            memory: ByteSize::gib(3),
            timeout: Duration::from_mins(15),
            targets: crate::DEFAULT_MAX_TARGETS,
            networking: false,
            max_log_size: ByteSize::kib(100),
        }
    }
}

impl Limits {
    pub fn from_config(config: &Config) -> Self {
        let mut limits = Limits::default();

        if let Some(memory_limit) = config.build_default_memory_limit {
            limits.memory = memory_limit;
        }

        limits
    }

    pub async fn for_crate(
        config: &Config,
        conn: &mut sqlx::PgConnection,
        name: &KrateName,
    ) -> Result<Self> {
        let default = Self::from_config(config);
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

    pub fn memory(&self) -> ByteSize {
        self.memory
    }

    pub fn timeout(&self) -> Duration {
        self.timeout
    }

    pub fn networking(&self) -> bool {
        self.networking
    }

    pub fn max_log_size(&self) -> ByteSize {
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
            memory: (defaults.memory.0 * 2).into(),
            timeout: (defaults.timeout.0 * 2).into(),
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
                timeout: Some(Duration::from_mins(20)),
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
            build_default_memory_limit: Some(ByteSize::gib(6)),
        };

        let mut conn = db.async_conn().await?;

        let limits = Limits::for_crate(&cfg, &mut conn, &KRATE).await?;
        assert_eq!(limits.memory, ByteSize::gib(6));

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
                memory: Some(ByteSize::b(defaults.memory.as_u64() / 2)),
                ..Overrides::default()
            },
        )
        .await?;

        let limits = Limits::for_crate(&cfg, &mut conn, &KRATE).await?;
        assert_eq!(limits, defaults);

        Ok(())
    }
}
