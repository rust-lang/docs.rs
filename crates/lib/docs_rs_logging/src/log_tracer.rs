use log::{Level, Log, Metadata, Record};

/// Forwards records from the [`log`] facade as tracing events containing only
/// their level and formatted message.
///
/// Unlike `tracing_log::LogTracer`, this deliberately omits origin fields
/// such as `log.target`, `log.module_path`, `log.file`, and `log.line`. Events
/// retain the currently entered tracing span context and use `rustwide` as
/// their tracing target.
#[derive(Debug, Default)]
pub struct MessageOnlyLogTracer;

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
