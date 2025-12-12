use anyhow::{Context as _, Result};
use clap::Parser;
use docs_rs_build_queue::AsyncBuildQueue;
use docs_rs_database::Pool;
use docs_rs_opentelemetry::AnyMeterProvider;
use docs_rs_utils::start_async_cron;
use std::{net::SocketAddr, sync::Arc, time::Duration};
use tracing::{info, trace};

#[derive(Parser)]
#[command(
    about = env!("CARGO_PKG_DESCRIPTION"),
    version = docs_rs_utils::BUILD_VERSION,
    rename_all = "kebab-case",
)]
struct Cli {
    #[arg(name = "SOCKET_ADDR", default_value = "0.0.0.0:3000")]
    socket_addr: SocketAddr,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let _guard = docs_rs_logging::init().context("error initializing logging")?;

    let args = Cli::try_parse().context("error parsing command line args")?;

    let context = docs_rs_context::Context::new()?
        .with_pool()
        .await?
        .with_build_queue()
        .await?
        .with_storage()
        .await?
        .with_registry_api()
        .await?;

    let config = Arc::new(crate::Config::from_environment()?);

    start_web_server(args.socket_addr, config, &context).await?;

    Ok(())
}
