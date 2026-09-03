use docs_rs_logging::{Config, MessageOnlyLogTracer};

pub fn init(config: &Config) {
    if config.log_build_logs {
        rustwide::logging::init_with(MessageOnlyLogTracer);
    } else {
        rustwide::logging::init();
    }
}
