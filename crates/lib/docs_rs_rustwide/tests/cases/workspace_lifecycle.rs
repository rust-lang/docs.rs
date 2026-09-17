use crate::support::{build_local, test_workspace};
use anyhow::Result;
use docs_rs_rustwide::BuildEnvironment;
use std::time::Duration;

#[test]
#[ignore = "requires Docker, network access, and a Rust toolchain"]
fn refreshes_workspace_when_interval_is_zero() -> Result<()> {
    let workspace = test_workspace();
    let mut environment = BuildEnvironment::builder(workspace.as_path())
        .wait_for_workspace_lock(true)
        .fast_init(true)
        .validate_host_resources(false)
        .sandbox_image(docs_rs_rustwide::testing::test_sandbox_image())
        .workspace_reinitialization_interval(Duration::ZERO)
        .build()?;

    let maintenance = environment.perform_maintenance()?;
    assert!(maintenance.workspace_refreshed);
    assert!(!maintenance.toolchain_updated);
    assert!(
        build_local(&mut environment, "build-std")?
            .into_inner()
            .build_succeeded()
    );
    Ok(())
}

#[test]
#[ignore = "requires Docker, network access, and a Rust toolchain"]
fn refreshes_workspace_after_interval() -> Result<()> {
    let workspace = test_workspace();
    let mut environment = BuildEnvironment::builder(workspace.as_path())
        .wait_for_workspace_lock(true)
        .fast_init(true)
        .validate_host_resources(false)
        .sandbox_image(docs_rs_rustwide::testing::test_sandbox_image())
        .workspace_reinitialization_interval(Duration::from_secs(1))
        .build()?;

    assert!(
        build_local(&mut environment, "hello-world")?
            .into_inner()
            .build_succeeded()
    );
    std::thread::sleep(Duration::from_secs(1));
    let maintenance = environment.perform_maintenance()?;
    assert!(maintenance.workspace_refreshed);
    assert!(!maintenance.toolchain_updated);
    assert!(
        build_local(&mut environment, "hello-world")?
            .into_inner()
            .build_succeeded()
    );
    Ok(())
}

#[test]
#[ignore = "requires Docker and a Rust toolchain"]
fn maintenance_is_disabled_by_default_and_preserves_artifacts() -> Result<()> {
    use crate::support::TestEnvironment;
    use docs_rs_rustwide::StepResultExt as _;
    let mut test = TestEnvironment::new()?;
    let release = build_local(&mut test.environment, "hello-world")?.into_inner();
    let html = release
        .default_target()
        .documentation()
        .as_inner()
        .unwrap()
        .path()
        .to_owned();
    let json = release
        .default_target()
        .rustdoc_json()
        .as_inner()
        .unwrap()
        .path()
        .to_owned();
    let json_contents = std::fs::read(&json)?;
    for _ in 0..2 {
        let maintenance = test.environment.perform_maintenance()?;
        assert!(!maintenance.workspace_refreshed);
        assert!(!maintenance.toolchain_updated);
        assert!(html.is_dir());
        assert_eq!(std::fs::read(&json)?, json_contents);
    }
    Ok(())
}
