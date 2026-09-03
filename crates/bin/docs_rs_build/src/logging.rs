use crate::args::ColorChoice;
use anyhow::{Result, anyhow};
use std::io::{IsTerminal as _, stdout};
use tracing::level_filters::LevelFilter;
use tracing_log::LogTracer;

pub(crate) fn init(verbosity: u8, color: ColorChoice) -> Result<()> {
    let level = match verbosity {
        0 => LevelFilter::INFO,
        1 => LevelFilter::DEBUG,
        _ => LevelFilter::TRACE,
    };
    let ansi = match color {
        ColorChoice::Always => true,
        ColorChoice::Never => false,
        ColorChoice::Auto => stdout().is_terminal(),
    };

    tracing_subscriber::fmt()
        .with_max_level(level)
        .with_target(verbosity > 0)
        .with_ansi(ansi)
        .with_writer(stdout)
        .try_init()
        .map_err(|error| anyhow!("initializing tracing output: {error}"))?;

    // Rustwide captures every build record in its StepResult and also forwards
    // it through this logger, which gives local and CI users live output.
    rustwide::logging::init_with(LogTracer::new());
    Ok(())
}
