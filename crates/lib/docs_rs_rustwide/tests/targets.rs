mod support;

use anyhow::Result;
use rustwide::Crate;
use support::{TestEnvironment, build_local};

#[test]
#[ignore = "requires Docker and a Rust toolchain"]
fn builds_metadata_and_default_targets() -> Result<()> {
    let mut test = TestEnvironment::with_default_targets()?;
    let release = build_local(&mut test.environment, "additional-targets")?.into_inner();
    let targets: Vec<_> = release
        .targets
        .iter()
        .map(|result| result.target.as_str())
        .collect();

    assert!(targets.contains(&"x86_64-apple-darwin"));
    assert!(targets.contains(&"aarch64-apple-darwin"));
    Ok(())
}

#[test]
#[ignore = "requires Docker, network access, and a Rust toolchain"]
fn cross_compiles_non_host_default_target() -> Result<()> {
    let mut test = TestEnvironment::with_default_targets()?;
    if test.environment.toolchain().as_ci().is_some() {
        return Ok(());
    }

    let krate = Crate::crates_io("windows-win", "2.4.1");
    let release = test
        .environment
        .release(&krate)
        .run(|build| build.build_docs())?
        .into_inner();
    let host = release
        .targets
        .iter()
        .find(|result| result.target == "x86_64-unknown-linux-gnu")
        .expect("host target should be included");

    assert!(host.successful());
    Ok(())
}

#[test]
#[ignore = "requires Docker and a nightly Rust toolchain"]
fn builds_with_build_std() -> Result<()> {
    let mut test = TestEnvironment::new()?;
    let release = build_local(&mut test.environment, "build-std")?.into_inner();
    assert!(release.successful());
    Ok(())
}
