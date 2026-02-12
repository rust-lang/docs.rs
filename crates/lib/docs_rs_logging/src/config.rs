use crate::LogFormat;
use docs_rs_config::AppConfig;
use docs_rs_env_vars::{env, maybe_env};
use std::str::FromStr;
use tracing_subscriber::{EnvFilter, filter::Directive};

#[derive(Debug)]
pub struct SentryConfig {
    pub dsn: sentry::types::Dsn,
    pub traces_sample_rate: f32,
}

#[derive(Debug)]
pub struct Config {
    pub format: LogFormat,
    pub filter: EnvFilter,
    pub sentry: Option<SentryConfig>,

    /// Whether to output the build logs to stdout too,
    /// or just store them on S3.
    pub log_build_logs: bool,
}

impl Config {
    fn filter_from_env(default_directive: &str) -> anyhow::Result<EnvFilter> {
        Ok(EnvFilter::builder()
            .with_default_directive(Directive::from_str(default_directive)?)
            .with_env_var("DOCSRS_LOG")
            .from_env_lossy())
    }
}

impl AppConfig for Config {
    fn from_environment() -> anyhow::Result<Self> {
        Ok(Self {
            format: maybe_env("DOCSRS_LOG_FORMAT")?.unwrap_or_default(),
            filter: Self::filter_from_env("info")?,
            sentry: maybe_env("SENTRY_DSN")?.map(|dsn| SentryConfig {
                dsn,
                traces_sample_rate: env("SENTRY_TRACES_SAMPLE_RATE", 0.0).unwrap_or(0.0),
            }),
            log_build_logs: env("DOCSRS_LOG_BUILD_LOGS", true)?,
        })
    }

    #[cfg(any(test, feature = "testing"))]
    fn test_config() -> anyhow::Result<Self> {
        Ok(Self {
            format: LogFormat::Pretty,
            filter: Self::filter_from_env("trace")?,
            sentry: None,
            log_build_logs: true,
        })
    }
}
