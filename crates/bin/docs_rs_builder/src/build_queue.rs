use crate::BuilderMetrics;
use crate::{PackageKind, RustwideBuilder};
use anyhow::Result;
use docs_rs_build_queue::{BuildPackageSummary, QueuedCrate};
use docs_rs_context::Context;
use docs_rs_fastly::CdnBehaviour as _;
use docs_rs_logging::BUILD_PACKAGE_TRANSACTION_NAME;
use docs_rs_utils::retry;
use std::time::Instant;
use tracing::{error, info_span};

/// wrapper around BuildQueue::process_next_crate to handle metrics and cdn invalidation
fn process_next_crate(
    context: &Context,
    builder_metrics: &BuilderMetrics,
    f: impl FnOnce(&QueuedCrate) -> Result<BuildPackageSummary>,
) -> Result<()> {
    let queue = context.blocking_build_queue()?.clone();
    let cdn = context.cdn();
    let runtime = context.runtime().clone();
    let queue_config = context.config().build_queue()?;

    let next_attempt = queue.process_next_crate(|to_process| {
        let res = {
            let instant = Instant::now();
            let res = f(to_process);
            let elapsed = instant.elapsed().as_secs_f64();
            builder_metrics.build_time.record(elapsed, &[]);
            res
        };

        builder_metrics.total_builds.add(1, &[]);

        if let Some(cdn) = cdn {
            runtime.block_on(cdn.queue_crate_invalidation(&to_process.name))?;
        }

        res
    })?;

    if let Some(next_attempt) = next_attempt
        && next_attempt >= queue_config.build_attempts as i32
    {
        builder_metrics.failed_builds.add(1, &[]);
    }

    Ok(())
}

pub(crate) fn build_next_queue_package(
    context: &Context,
    builder: &mut RustwideBuilder,
) -> Result<bool> {
    let mut processed = false;
    let queue = context.blocking_build_queue()?.clone();

    process_next_crate(context, &builder.builder_metrics.clone(), |krate| {
        let _span = info_span!(
            parent: None,
            BUILD_PACKAGE_TRANSACTION_NAME,
            crate_name = %krate.name,
            crate_version = %krate.version,
            attempt = krate.attempt,
        )
        .entered();

        processed = true;

        let kind = krate
            .registry
            .as_ref()
            .map(|r| PackageKind::Registry(r.as_str()))
            .unwrap_or(PackageKind::CratesIo);

        if let Err(err) = retry(|| builder.reinitialize_workspace_if_interval_passed(), 3) {
            error!(?err, "Reinitialize workspace failed after retries");
            queue.lock()?;
            return Err(err);
        }

        if let Err(err) = builder.update_toolchain_and_add_essential_files() {
            error!(?err, "Updating toolchain failed, locking queue");
            queue.lock()?;
            return Err(err);
        }

        builder.build_package(&krate.name, &krate.version, kind, krate.attempt == 0)
    })?;

    Ok(processed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::TestEnvironment;
    use docs_rs_build_queue::BuildPackageSummary;
    use docs_rs_headers::SurrogateKey;
    use docs_rs_types::{KrateName, testing::V1};
    use pretty_assertions::assert_eq;

    #[test]
    fn test_invalidate_cdn_after_error() -> Result<()> {
        let env = TestEnvironment::new()?;
        let queue = env.blocking_build_queue()?;

        let builder_metrics = BuilderMetrics::new(env.meter_provider());

        const WILL_FAIL: KrateName = KrateName::from_static("will_fail");

        queue.add_crate(&WILL_FAIL, &V1, 0, None)?;

        process_next_crate(&env, &builder_metrics, |krate| {
            assert_eq!(WILL_FAIL, krate.name);
            anyhow::bail!("simulate a failure");
        })?;

        assert_eq!(
            env.runtime().block_on(env.cdn().purged_keys()).unwrap(),
            SurrogateKey::from(WILL_FAIL).into()
        );

        Ok(())
    }

    #[test]
    fn test_invalidate_cdn_after_build() -> Result<()> {
        let env = TestEnvironment::new()?;
        let queue = env.blocking_build_queue()?;
        let builder_metrics = BuilderMetrics::new(env.meter_provider());

        const WILL_SUCCEED: KrateName = KrateName::from_static("will_succeed");
        queue.add_crate(&WILL_SUCCEED, &V1, -1, None)?;

        process_next_crate(&env, &builder_metrics, |krate| {
            assert_eq!(WILL_SUCCEED, krate.name);
            Ok(BuildPackageSummary::default())
        })?;

        assert_eq!(
            env.runtime().block_on(env.cdn().purged_keys()).unwrap(),
            SurrogateKey::from(WILL_SUCCEED).into()
        );

        Ok(())
    }
}
