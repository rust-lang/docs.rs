use anyhow::{Result, anyhow};
use std::io::stdout;
use tracing::level_filters::LevelFilter;

pub(crate) fn init(verbosity: u8) -> Result<()> {
    let level = match verbosity {
        0 => LevelFilter::INFO,
        1 => LevelFilter::DEBUG,
        _ => LevelFilter::TRACE,
    };

    tracing_subscriber::fmt()
        .compact()
        .with_max_level(level)
        .with_target(verbosity > 0)
        .with_ansi(true)
        .with_writer(stdout)
        .try_init()
        .map_err(|error| anyhow!("initializing tracing output: {error}"))?;

    // Rustwide captures every build record in its StepResult and also forwards
    // it through this logger, which gives local and CI users live output.
    docs_rs_rustwide::logging::init(true);
    Ok(())
}
