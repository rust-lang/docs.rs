pub(crate) mod common;
pub(crate) mod crates_io;
mod import;
mod rustdoc;
pub(crate) mod rustdoc_status;

use anyhow::{Context as _, Result};
use clap::Parser;
use docs_rs_context::Context;
use docs_rs_types::{KrateName, ReqVersion};

#[tokio::main]
async fn main() -> Result<()> {
    let _guard = docs_rs_logging::init().context("error initializing logging")?;

    if let Err(err) = CommandLine::parse().handle_args().await {
        eprintln!("error importing release: {err:?}");
        drop(_guard);
        std::process::exit(1);
    }

    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq, Parser)]
#[command(
    about = env!("CARGO_PKG_DESCRIPTION"),
    version = docs_rs_utils::BUILD_VERSION,
    rename_all = "kebab-case",
)]
struct CommandLine {
    #[arg(name = "CRATE")]
    name: KrateName,

    #[arg(name = "CRATE_VERSION", default_value_t)]
    version: ReqVersion,
}

impl CommandLine {
    async fn handle_args(self) -> Result<()> {
        let ctx = Context::builder()
            .with_runtime()
            .await?
            .with_meter_provider()?
            .with_pool()
            .await?
            .with_storage()
            .await?
            .with_registry_api()?
            .with_repository_stats()?
            .build()?;

        let mut conn = ctx.pool()?.get_async().await?;
        import::import_test_release(
            &mut conn,
            ctx.storage()?,
            ctx.registry_api()?,
            ctx.repository_stats()?,
            &self.name,
            &self.version,
        )
        .await
    }
}
