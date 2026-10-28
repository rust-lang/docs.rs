use anyhow::Context as _;
use clap::Parser;
use docs_rs_config::AppConfig as _;
use docs_rs_web::{Config, build_context, run_web_server};
use std::{net::SocketAddr, sync::Arc};

#[derive(Parser)]
#[command(
    about = env!("CARGO_PKG_DESCRIPTION"),
    version = docs_rs_utils::BUILD_VERSION,
    rename_all = "kebab-case",
)]
struct Cli {
    /// Listen address (defaults to 0.0.0.0 on DOCSRS_WEB_PORT, or port 3000).
    #[arg(name = "SOCKET_ADDR")]
    socket_addr: Option<SocketAddr>,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let _guard = docs_rs_logging::init_from_environment().context("error initializing logging")?;

    let args = Cli::parse();
    let socket_addr = match args.socket_addr {
        Some(addr) => addr,
        None => SocketAddr::from((
            [0, 0, 0, 0],
            docs_rs_env_vars::env("DOCSRS_WEB_PORT", 3000)?,
        )),
    };
    let context = build_context().await?;
    let config = Arc::new(Config::from_environment()?);

    run_web_server(Some(socket_addr), config, context).await?;

    Ok(())
}
