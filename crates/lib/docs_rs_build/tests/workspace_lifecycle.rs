mod support;

use anyhow::Result;
use docs_rs_build::BuildEnvironment;
use std::time::Duration;
use support::{build_local, fixture, test_sandbox_image};

#[test]
#[ignore = "requires Docker, network access, and a Rust toolchain"]
fn refreshes_workspace_when_interval_is_zero() -> Result<()> {
    let workspace = tempfile::tempdir()?;
    let mut environment = BuildEnvironment::builder(workspace.path())
        .fast_init(true)
        .validate_host_resources(false)
        .sandbox_image(test_sandbox_image())
        .workspace_reinitialization_interval(Duration::ZERO)
        .build()?;

    let maintenance = environment.perform_maintenance()?;
    assert!(maintenance.workspace_refreshed);
    assert!(
        build_local(&mut environment, "build-std")?
            .into_inner()
            .successful()
    );
    Ok(())
}

#[test]
#[ignore = "requires Docker, network access, and a Rust toolchain"]
fn refreshes_workspace_after_interval() -> Result<()> {
    let workspace = tempfile::tempdir()?;
    let mut environment = BuildEnvironment::builder(workspace.path())
        .fast_init(true)
        .validate_host_resources(false)
        .sandbox_image(test_sandbox_image())
        .workspace_reinitialization_interval(Duration::from_secs(1))
        .build()?;

    assert!(
        build_local(&mut environment, "hello-world")?
            .into_inner()
            .successful()
    );
    std::thread::sleep(Duration::from_secs(1));
    assert!(environment.perform_maintenance()?.workspace_refreshed);
    assert!(
        build_local(&mut environment, "hello-world")?
            .into_inner()
            .successful()
    );
    Ok(())
}

#[test]
#[ignore = "requires Docker and a Rust toolchain"]
fn recreated_environment_uses_existing_toolchain() -> Result<()> {
    let workspace = tempfile::tempdir()?;
    let old_version = {
        let environment = BuildEnvironment::builder(workspace.path())
            .fast_init(true)
            .validate_host_resources(false)
            .sandbox_image(test_sandbox_image())
            .build()?;
        environment.rustc_version()?
    };

    let mut environment = BuildEnvironment::builder(workspace.path())
        .fast_init(true)
        .validate_host_resources(false)
        .sandbox_image(test_sandbox_image())
        .build()?;
    let fixture = fixture("hello-world");
    let krate = rustwide::Crate::local(&fixture);
    assert!(
        environment
            .release(&krate)
            .run(|build| build.build_docs())?
            .into_inner()
            .successful()
    );
    assert_eq!(old_version, environment.rustc_version()?);
    Ok(())
}
