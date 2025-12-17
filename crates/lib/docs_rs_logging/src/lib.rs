#[cfg(feature = "testing")]
pub mod testing;

use sentry::{
    TransactionContext, integrations::panic as sentry_panic,
    integrations::tracing as sentry_tracing,
};
use std::{env, str::FromStr as _, sync::Arc};
use tracing_subscriber::{EnvFilter, filter::Directive, prelude::*};

pub struct Guard {
    #[allow(dead_code)]
    sentry_guard: Option<sentry::ClientInitGuard>,
}

pub fn init() -> anyhow::Result<Guard> {
    let log_formatter = {
        let log_format = env::var("DOCSRS_LOG_FORMAT").unwrap_or_default();

        if log_format == "json" {
            tracing_subscriber::fmt::layer().json().boxed()
        } else {
            tracing_subscriber::fmt::layer().boxed()
        }
    };

    let tracing_registry = tracing_subscriber::registry().with(log_formatter).with(
        EnvFilter::builder()
            .with_default_directive(Directive::from_str("docs_rs=info")?)
            .with_env_var("DOCSRS_LOG")
            .from_env_lossy(),
    );

    let sentry_guard = if let Ok(sentry_dsn) = env::var("SENTRY_DSN") {
        tracing::subscriber::set_global_default(tracing_registry.with(
            sentry_tracing::layer().event_filter(|md| {
                if md.fields().field("reported_to_sentry").is_some() {
                    sentry_tracing::EventFilter::Ignore
                } else {
                    sentry_tracing::default_event_filter(md)
                }
            }),
        ))?;

        let traces_sample_rate = env::var("SENTRY_TRACES_SAMPLE_RATE")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(0.0);

        let traces_sampler = move |ctx: &TransactionContext| -> f32 {
            if let Some(sampled) = ctx.sampled() {
                // if the transaction was already marked as "to be sampled" by
                // the JS/frontend SDK, we want to sample it in the backend too.
                return if sampled { 1.0 } else { 0.0 };
            }

            let op = ctx.operation();
            if op == "docbuilder.build_package" {
                // record all transactions for builds
                1.
            } else {
                traces_sample_rate
            }
        };

        Some(sentry::init((
            sentry_dsn,
            sentry::ClientOptions {
                release: Some(docs_rs_utils::BUILD_VERSION.into()),
                attach_stacktrace: true,
                traces_sampler: Some(Arc::new(traces_sampler)),
                ..Default::default()
            }
            .add_integration(sentry_panic::PanicIntegration::default()),
        )))
    } else {
        tracing::subscriber::set_global_default(tracing_registry)?;
        None
    };

    Ok(Guard { sentry_guard })
}
