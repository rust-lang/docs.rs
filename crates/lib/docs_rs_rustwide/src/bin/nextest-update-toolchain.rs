use anyhow::{Context as _, Result};
use docs_rs_rustwide::{BuildEnvironment, testing};

/// Installs or updates the toolchain in the shared test workspace once per
/// nextest run, before any builder test starts.
///
/// Without this, a local workspace keeps whatever nightly it first installed,
/// while CI always starts from an empty workspace with the latest nightly.
fn main() -> Result<()> {
    docs_rs_rustwide::logging::init(false);

    let workspace = testing::test_workspace_path();
    let mut environment = BuildEnvironment::builder(workspace.as_path())
        .wait_for_workspace_lock(true)
        .fast_init(true)
        .validate_host_resources(false)
        .sandbox_image(testing::test_sandbox_image())
        .build()
        .context("preparing the test build environment")?;

    let changed = environment
        .update_toolchain()
        .context("updating the test toolchain")?;
    let version = environment.rustc_version()?;

    // Release the workspace lock before the tests start.
    drop(environment);

    println!(
        "test toolchain {}: {version}",
        if changed { "updated" } else { "unchanged" }
    );
    Ok(())
}
