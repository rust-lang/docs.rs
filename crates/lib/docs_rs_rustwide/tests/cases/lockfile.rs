use crate::support::{TestEnvironment, build_local};
use anyhow::Result;

#[test]
#[ignore = "requires Docker, network access, and a Rust toolchain"]
fn retries_with_a_new_lockfile_for_updated_dependencies() -> Result<()> {
    let mut test = TestEnvironment::new()?;
    let release = build_local(&mut test.environment, "incorrect_lockfile_0_1")?.into_inner();
    assert!(release.has_docs());
    assert!(
        release
            .default_target()
            .regenerate_lockfile()
            .is_some_and(Result::is_ok)
    );
    Ok(())
}

#[test]
#[ignore = "requires Docker, network access, and a Rust toolchain"]
fn retries_with_a_new_lockfile_for_new_dependencies() -> Result<()> {
    let mut test = TestEnvironment::new()?;
    let release = build_local(&mut test.environment, "incorrect_lockfile_0_2")?.into_inner();
    assert!(release.has_docs());
    assert!(
        release
            .default_target()
            .regenerate_lockfile()
            .is_some_and(Result::is_ok)
    );
    Ok(())
}
