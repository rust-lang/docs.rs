mod support;

use anyhow::Result;
use support::{TestEnvironment, build_local};

#[test]
#[ignore = "requires Docker, network access, and a Rust toolchain"]
fn retries_with_a_new_lockfile_for_updated_dependencies() -> Result<()> {
    let mut test = TestEnvironment::new()?;
    let release = build_local(&mut test.environment, "incorrect_lockfile_0_1")?.into_inner();
    assert!(release.successful());
    Ok(())
}

#[test]
#[ignore = "requires Docker, network access, and a Rust toolchain"]
fn retries_with_a_new_lockfile_for_new_dependencies() -> Result<()> {
    let mut test = TestEnvironment::new()?;
    let release = build_local(&mut test.environment, "incorrect_lockfile_0_2")?.into_inner();
    assert!(release.successful());
    Ok(())
}
