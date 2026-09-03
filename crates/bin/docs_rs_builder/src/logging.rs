use docs_rs_logging::Config;
use log::{Level, Log, Metadata, Record};

pub fn init(config: &Config) {
    if config.log_build_logs {
        rustwide::logging::init_with(RustwideLogTracer);
    } else {
        rustwide::logging::init();
    }
}

/// Forwards Rustwide's `log` records as tracing events containing their level,
/// formatted message, and original log target.
///
/// Unlike `tracing_log::LogTracer`, this deliberately omits origin fields
/// such as `log.module_path`, `log.file`, and `log.line`. Events retain the
/// currently entered tracing span context, use `rustwide` as their tracing target,
/// and preserve the original target in the `log.target` field.
///
/// Later we'll migrate rustwide to directly emitting tracing-events.
#[derive(Debug, Default)]
struct RustwideLogTracer;

impl Log for RustwideLogTracer {
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
            Level::Error => tracing::event!(
                target: "rustwide", tracing::Level::ERROR,
                { "log.target" = record.target(), message = format_args!("{}", record.args()) }
            ),
            Level::Warn => tracing::event!(
                target: "rustwide", tracing::Level::WARN,
                { "log.target" = record.target(), message = format_args!("{}", record.args()) }
            ),
            Level::Info => tracing::event!(
                target: "rustwide", tracing::Level::INFO,
                { "log.target" = record.target(), message = format_args!("{}", record.args()) }
            ),
            Level::Debug => tracing::event!(
                target: "rustwide", tracing::Level::DEBUG,
                { "log.target" = record.target(), message = format_args!("{}", record.args()) }
            ),
            Level::Trace => tracing::event!(
                target: "rustwide", tracing::Level::TRACE,
                { "log.target" = record.target(), message = format_args!("{}", record.args()) }
            ),
        }
    }

    fn flush(&self) {}
}
