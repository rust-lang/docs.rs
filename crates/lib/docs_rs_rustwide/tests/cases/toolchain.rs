use crate::support::{fixture, test_workspace};
use anyhow::Result;
use docs_rs_rustwide::BuildEnvironment;

#[test]
#[ignore = "requires Docker and a Rust toolchain"]
fn recreated_environment_uses_existing_toolchain() -> Result<()> {
    let workspace = test_workspace();
    let old_version = {
        let environment = BuildEnvironment::builder(workspace.as_path())
            .wait_for_workspace_lock(true)
            .fast_init(true)
            .validate_host_resources(false)
            .sandbox_image(docs_rs_rustwide::testing::test_sandbox_image())
            .build()?;
        environment.rustc_version()?
    };

    let mut environment = BuildEnvironment::builder(workspace.as_path())
        .wait_for_workspace_lock(true)
        .fast_init(true)
        .validate_host_resources(false)
        .sandbox_image(docs_rs_rustwide::testing::test_sandbox_image())
        .build()?;
    let fixture = fixture("hello-world");
    let krate = rustwide::Crate::local(&fixture);
    assert!(
        environment
            .release(&krate)
            .run(|build| Ok(build.build_docs()))?
            .into_inner()
            .build_succeeded()
    );
    assert_eq!(old_version, environment.rustc_version()?);
    Ok(())
}
