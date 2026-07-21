use docs_rs_logging::Config;
use tracing_log::LogTracer;

pub fn init(config: &Config) {
    if config.log_build_logs {
        rustwide::logging::init_with(LogTracer::new());
    } else {
        rustwide::logging::init();
    }
}
