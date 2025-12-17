use std::str::FromStr as _;
use tracing_subscriber::{EnvFilter, filter::Directive};

pub fn init() {
    let subscriber = tracing_subscriber::FmtSubscriber::builder()
        .with_env_filter(
            EnvFilter::builder()
                .with_default_directive(Directive::from_str("docs_rs=info").unwrap())
                .with_env_var("DOCSRS_LOG")
                .from_env_lossy(),
        )
        .with_test_writer()
        .finish();
    let _ = tracing::subscriber::set_global_default(subscriber);
}
