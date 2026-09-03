use anyhow::{Result, anyhow};
use log::{Level, Log, Metadata, Record};
use std::io::stdout;
use tracing::level_filters::LevelFilter;

/// Forwards `log` records as tracing events without `tracing-log`'s additional
/// `log.target`, `log.module_path`, `log.file`, and `log.line` fields.
struct MessageOnlyLogTracer;

impl Log for MessageOnlyLogTracer {
    fn enabled(&self, metadata: &Metadata<'_>) -> bool {
        match metadata.level() {
            Level::Error => tracing::enabled!(target: "rustwide", tracing::Level::ERROR),
            Level::Warn => tracing::enabled!(target: "rustwide", tracing::Level::WARN),
            Level::Info => tracing::enabled!(target: "rustwide", tracing::Level::INFO),
            Level::Debug => tracing::enabled!(target: "rustwide", tracing::Level::DEBUG),
            Level::Trace => tracing::enabled!(target: "rustwide", tracing::Level::TRACE),
        }
    }

    fn log(&self, record: &Record<'_>) {
        if !self.enabled(record.metadata()) {
            return;
        }

        match record.level() {
            Level::Error => tracing::error!(target: "rustwide", "{}", record.args()),
            Level::Warn => tracing::warn!(target: "rustwide", "{}", record.args()),
            Level::Info => tracing::info!(target: "rustwide", "{}", record.args()),
            Level::Debug => tracing::debug!(target: "rustwide", "{}", record.args()),
            Level::Trace => tracing::trace!(target: "rustwide", "{}", record.args()),
        }
    }

    fn flush(&self) {}
}

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
    rustwide::logging::init_with(MessageOnlyLogTracer);
    Ok(())
}
