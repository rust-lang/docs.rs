use anyhow::Context as _;
use docs_rs_builder::{
    Config, docbuilder::rustwide_builder::RustwideBuilder, utils::queue_builder::queue_builder,
};
use std::sync::Arc;
use tokio::runtime;
use tracing_log::LogTracer;

fn main() -> anyhow::Result<()> {
    let _guard = docs_rs_logging::init().context("error initializing logging")?;

    // set the global log::logger for backwards compatibility
    // through rustwide.
    rustwide::logging::init_with(LogTracer::new());

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;

    let context = runtime.block_on(async {
        // handle for the current runtime from above.
        let handle = runtime::Handle::current();
        docs_rs_context::Context::new_with_runtime(handle)?
                .with_pool()
                .await?
                .with_storage()
                .await?
                .with_cdn()
                .await?
                .with_build_queue()
                .await
    })?;

    let config = Arc::new(Config::from_environment()?);

    queue_builder(
        &config,
        &context,
        RustwideBuilder::init(config.clone(), &context)?,
    )?;

    Ok(())
}
