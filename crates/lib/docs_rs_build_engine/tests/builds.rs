mod support;

use anyhow::{Context as _, Result};
use docs_rs_build_engine::{BuildEnvironment, CpuLimit};
use rustwide::Crate;
use std::fs;
use support::{TestEnvironment, build_local, fixture, test_sandbox_image};
use test_case::test_case;

#[test]
#[ignore = "requires Docker and a Rust toolchain"]
fn builds_library_documentation_json_and_coverage() -> Result<()> {
    let mut test = TestEnvironment::new()?;
    let build = build_local(&mut test.environment, "hello-world")?;
    assert!(
        build
            .statistics()
            .memory_peak_bytes()
            .is_some_and(|v| v > 0)
    );

    let release = build.into_inner();
    assert!(release.successful());
    assert!(release.has_docs());
    assert!(release.default_target().rustdoc_json.successful());
    assert!(release.default_target().coverage.successful());
    assert!(
        release
            .default_target()
            .coverage
            .output
            .as_ref()
            .is_some_and(Option::is_some)
    );
    assert!(
        release
            .default_target()
            .rustdoc_json
            .output
            .as_ref()
            .expect("successful JSON build has an output")
            .format_version()
            .is_ok()
    );
    Ok(())
}

#[test]
#[ignore = "requires Docker, network access, and a Rust toolchain"]
fn binary_crate_does_not_report_library_documentation() -> Result<()> {
    let mut test = TestEnvironment::new()?;
    let krate = Crate::crates_io("heater", "0.2.3");
    let release = test
        .environment
        .release(&krate)
        .run(|build| build.build_docs())?
        .into_inner();

    assert!(!release.cargo_metadata.root().is_library());
    assert!(!release.has_docs());
    Ok(())
}

#[test_case("scsys-macros", "0.2.6")]
#[test_case("scsys-derive", "0.2.6")]
#[test_case("thiserror-impl", "1.0.26")]
#[test_case("contained-macros", "0.2.5")]
#[ignore = "requires Docker, network access, and a Rust toolchain"]
fn builds_proc_macro(crate_name: &str, version: &str) -> Result<()> {
    let mut test = TestEnvironment::new()?;
    let krate = Crate::crates_io(crate_name, version);
    let release = test
        .environment
        .release(&krate)
        .run(|build| build.build_docs())?
        .into_inner();

    assert!(release.successful());
    assert!(release.has_docs());
    assert!(release.default_target().coverage.successful());
    assert!(release.default_target().rustdoc_json.successful());
    Ok(())
}

#[test]
#[ignore = "requires Docker, network access, and a Rust toolchain"]
fn passes_rustflags_to_build_scripts() -> Result<()> {
    let mut test = TestEnvironment::new()?;
    let krate = Crate::crates_io("proc-macro2", "1.0.95");
    let release = test
        .environment
        .release(&krate)
        .run(|build| build.build_docs())?
        .into_inner();
    assert!(release.successful());
    Ok(())
}

#[test]
#[ignore = "requires Docker and a Rust toolchain"]
fn builds_coverage_and_json_for_crates_with_examples() -> Result<()> {
    let mut test = TestEnvironment::new()?;
    let release = build_local(&mut test.environment, "with-examples")?.into_inner();

    assert!(release.successful());
    assert!(release.default_target().coverage.successful());
    assert!(
        release
            .default_target()
            .coverage
            .output
            .as_ref()
            .is_some_and(Option::is_some)
    );
    assert!(release.default_target().rustdoc_json.successful());
    Ok(())
}

#[test_case("ffizz-string", "0.5.0")]
#[test_case("ffizz-passby", "0.5.0")]
#[ignore = "requires Docker, network access, and a Rust toolchain"]
fn handles_crates_with_custom_scrape_examples(crate_name: &str, version: &str) -> Result<()> {
    let mut test = TestEnvironment::new()?;
    let krate = Crate::crates_io(crate_name, version);
    let release = test
        .environment
        .release(&krate)
        .run(|build| build.build_docs())?
        .into_inner();

    assert!(release.successful());
    assert!(release.default_target().coverage.successful());
    assert!(release.default_target().rustdoc_json.successful());
    Ok(())
}

#[test]
#[ignore = "requires Docker and a Rust toolchain"]
fn collects_compiler_metrics() -> Result<()> {
    let workspace = tempfile::tempdir()?;
    let metrics = tempfile::tempdir()?;
    let mut environment = BuildEnvironment::builder(workspace.path())
        .fast_init(true)
        .validate_host_resources(false)
        .sandbox_image(test_sandbox_image())
        .compiler_metrics_collection_path(metrics.path())
        .build()?;

    let release = build_local(&mut environment, "hello-world")?.into_inner();
    let metric_files = &release.default_target().compiler_metrics;
    assert_eq!(metric_files.len(), 1);
    let _: serde_json::Value = serde_json::from_slice(&fs::read(&metric_files[0])?)?;
    Ok(())
}

#[test_case(CpuLimit::Quota(2.0))]
#[test_case(CpuLimit::Cores(1..=2))]
#[ignore = "requires Docker and a Rust toolchain"]
fn builds_with_cpu_restrictions(cpu_limit: CpuLimit) -> Result<()> {
    let workspace = tempfile::tempdir()?;
    let mut environment = BuildEnvironment::builder(workspace.path())
        .fast_init(true)
        .validate_host_resources(false)
        .sandbox_image(test_sandbox_image())
        .cpu_limit(cpu_limit)
        .build()?;
    assert!(
        build_local(&mut environment, "hello-world")?
            .into_inner()
            .successful()
    );
    Ok(())
}

#[test]
#[ignore = "requires Docker and a Rust toolchain"]
fn source_can_be_copied_before_a_failed_build() -> Result<()> {
    let mut test = TestEnvironment::new()?;
    let destination = tempfile::tempdir()?;
    let fixture = fixture("simple-build-failure");
    let krate = Crate::local(&fixture);
    let release = test
        .environment
        .release(&krate)
        .fetch()?
        .try_inspect(|fetched| fetched.copy_source_to(destination.path()))?
        .run(|build| build.build_docs())?
        .into_inner();

    assert!(destination.path().join("src/main.rs").is_file());
    assert!(!release.successful());
    Ok(())
}

#[test]
#[ignore = "requires Docker, network access, and a Rust toolchain"]
fn reports_implicit_features_for_optional_dependencies() -> Result<()> {
    let mut test = TestEnvironment::new()?;
    let krate = Crate::crates_io("serde", "1.0.152");
    let release = test
        .environment
        .release(&krate)
        .run(|build| build.build_docs())?
        .into_inner();

    assert!(
        release
            .cargo_metadata
            .root()
            .features
            .contains_key("serde_derive")
    );
    Ok(())
}

#[test]
#[ignore = "requires Docker, network access, and a Rust toolchain"]
fn excludes_implicit_features_when_dep_syntax_is_used() -> Result<()> {
    let mut test = TestEnvironment::new()?;
    let release = build_local(&mut test.environment, "optional-dep")?.into_inner();
    let features: Vec<_> = release
        .cargo_metadata
        .root()
        .features
        .keys()
        .map(String::as_str)
        .collect();

    assert_eq!(features, ["alloc", "default", "optional_regex", "std"]);
    assert!(!features.contains(&"regex"));
    Ok(())
}

#[test]
#[ignore = "requires Docker, network access, and a Rust toolchain"]
fn reports_failure_before_sandbox_preparation() -> Result<()> {
    let mut test = TestEnvironment::new()?;
    let krate = Crate::crates_io("emheap", "0.1.0");
    let error = test
        .environment
        .release(&krate)
        .run(|build| build.build_docs())
        .err()
        .context("the published crate unexpectedly built")?;

    assert!(error.to_string().contains("Cargo.toml"));
    Ok(())
}
