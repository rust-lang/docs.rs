use anyhow::Context as _;
use docs_rs_config::AppConfig as _;
use docs_rs_database::{
    Config,
    testing::{TEMPLATE_DDL_ENV, prepare_template_schema},
};
use std::env;
use tokio::{fs, io::AsyncWriteExt as _};

/// Prepares the shared test schema once for a nextest run, then publishes the
/// captured DDL location to every test process.
#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let config = Config::from_environment().context("missing database config in env")?;
    let path = prepare_template_schema(&config).await?;

    let env_file = env::var("NEXTEST_ENV")
        .context("NEXTEST_ENV is not set (this binary must be run by nextest)")?;

    let mut file = fs::OpenOptions::new().append(true).open(env_file).await?;
    file.write_all(format!("{TEMPLATE_DDL_ENV}={}", path.display()).as_bytes())
        .await?;
    file.flush().await?;

    Ok(())
}
