use crate::support::{TestEnvironment, build_local};
use anyhow::Result;
use docs_rs_rustwide::StepResultExt as _;
use docsrs_metadata::HOST_TARGET;
use rustwide::Crate;

#[test]
#[ignore = "requires Docker and a Rust toolchain"]
fn builds_metadata_and_default_targets() -> Result<()> {
    let mut test = TestEnvironment::with_default_targets()?;
    let release = build_local(&mut test.environment, "additional-targets")?.into_inner();
    let targets: Vec<_> = release.targets().map(|result| result.target()).collect();

    assert!(targets.contains(&"x86_64-apple-darwin"));
    assert!(targets.contains(&"aarch64-apple-darwin"));
    assert_eq!(release.default_target().target(), HOST_TARGET);
    assert_eq!(
        targets
            .iter()
            .filter(|&&target| target == HOST_TARGET)
            .count(),
        1
    );
    assert_eq!(
        release
            .targets()
            .filter(|target| target.is_default())
            .count(),
        1
    );
    assert!(release.default_target().is_default());

    for target in release.targets() {
        assert!(
            target.has_docs("additional_targets"),
            "{} is missing its HTML documentation: {:?}",
            target.target(),
            target.documentation(),
        );
        let json = target
            .rustdoc_json()
            .as_ref()
            .unwrap_or_else(|error| panic!("{} JSON build failed: {error}", target.target()));
        assert!(
            json.value().format_version().is_ok(),
            "{} JSON is unreadable",
            target.target()
        );

        for (mode, log) in [
            ("HTML", target.documentation().log()),
            ("JSON", target.rustdoc_json().log()),
        ] {
            assert!(
                log.is_some_and(|log| !log.trim().is_empty()),
                "{} is missing its {mode} build log",
                target.target(),
            );
        }
    }
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
        .run(|build| Ok(build.build_docs()?))?
        .into_inner();
    let host = release
        .targets()
        .find(|result| result.target() == "x86_64-unknown-linux-gnu")
        .expect("host target should be included");

    assert!(host.documentation_succeeded());
    Ok(())
}

#[test]
#[ignore = "requires Docker and a nightly Rust toolchain"]
fn builds_with_build_std() -> Result<()> {
    let mut test = TestEnvironment::new()?;
    let release = build_local(&mut test.environment, "build-std")?.into_inner();
    assert!(release.build_succeeded());
    Ok(())
}
