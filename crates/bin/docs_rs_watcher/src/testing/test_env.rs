use crate::Config as WatcherConfig;
use anyhow::Result;
use crates_index_diff::gix::ObjectId;
use docs_rs_config::AppConfig as _;
use docs_rs_registry_api::testing::registry_entry;
use docs_rs_types::testing::{KRATE, V1, V2};

pub(crate) type TestEnvironment = docs_rs_context::testing::TestEnvironment<WatcherConfig>;

/// Set up history where HEAD unyanks V1 and publishes V2 after the checkpoint.
/// Git storage and sparse responses are both owned by the environment's registry.
pub(crate) async fn registry_history(
    env: &TestEnvironment,
) -> Result<(WatcherConfig, ObjectId, ObjectId)> {
    let registry = env.test_registry();
    let origin = registry.enable_git_index().await?;

    registry
        .mock_index_response(&KRATE, [registry_entry(&KRATE, &V1, None, true)])
        .await;

    let checkpoint = registry.git_head().await?;

    registry
        .mock_index_response(
            &KRATE,
            [
                registry_entry(&KRATE, &V1, None, false),
                registry_entry(&KRATE, &V2, None, false),
            ],
        )
        .await;

    let head = registry.git_head().await?;

    let mut config = WatcherConfig::test_config()?;
    config.registry_index_path = origin
        .to_file_path()
        .unwrap()
        .with_file_name("watcher-index");
    config.registry_url = Some(origin.to_string());

    Ok((config, checkpoint, head))
}
