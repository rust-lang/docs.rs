use docs_rs_env_vars::maybe_env;
use url::Url;

pub struct Config {
    // opentelemetry endpoint to send OTLP to
    pub endpoint: Option<Url>,
}

impl Config {
    pub fn from_environment() -> anyhow::Result<Self> {
        Ok(Self {
            endpoint: maybe_env("OTEL_EXPORTER_OTLP_ENDPOINT")?,
        })
    }
}
