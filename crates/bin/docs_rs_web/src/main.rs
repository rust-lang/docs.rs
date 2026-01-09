use anyhow::Context as _;
use clap::Parser;
use docs_rs_config::AppConfig as _;
use docs_rs_web::{Config, run_web_server};
use std::{net::SocketAddr, sync::Arc};

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

    let args = Cli::parse();

    let context = Arc::new(
        docs_rs_context::Context::builder()
            .with_runtime()
            .await?
            .with_meter_provider()?
            .with_pool()
            .await?
            .with_build_queue()?
            .with_storage()
            .await?
            .with_registry_api()?
            .with_build_limits()?
            .build()?,
    );

    let config = Arc::new(Config::from_environment()?);

    run_web_server(Some(args.socket_addr), config, context).await?;

    Ok(())
}
