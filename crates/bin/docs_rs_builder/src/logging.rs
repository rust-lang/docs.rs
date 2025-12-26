use tracing_log::LogTracer;

pub fn init() {
    rustwide::logging::init_with(LogTracer::new());
}
