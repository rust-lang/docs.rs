use anyhow::Result;
use docs_rs_config::AppConfig;
use docs_rs_env_vars::maybe_env;
use url::Url;

#[derive(Debug)]
pub struct Config {
    // opentelemetry endpoint to send OTLP to
    pub endpoint: Option<Url>,
}

impl AppConfig for Config {
    fn from_environment() -> Result<Self> {
        Ok(Self {
            endpoint: maybe_env("OTEL_EXPORTER_OTLP_ENDPOINT")?,
        })
    }
}
