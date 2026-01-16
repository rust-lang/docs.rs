use anyhow::Result;
use docs_rs_context::Context;
use std::sync::Arc;

pub async fn build_context() -> Result<Arc<Context>> {
    Ok(Arc::new(
        Context::builder()
            .with_runtime()
            .await?
            .with_meter_provider()?
            .with_pool()
            .await?
            .with_build_queue()?
            .with_storage()
            .await?
            .with_registry_api()?
            .with_build_limits()?
            .build()?,
    ))
}
