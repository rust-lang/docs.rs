use crate::support::{TestEnvironment, build_local, fixture, test_workspace};
use anyhow::{Context as _, Result};
use docs_rs_rustwide::{BuildEnvironment, CpuLimit, StepResultExt};
use rustwide::Crate;
use std::fs;
use test_case::test_case;

#[test_case(["html", "json", "coverage"])]
#[test_case(["json", "coverage", "html"])]
#[test_case(["coverage", "html", "json"])]
#[ignore = "requires Docker and a Rust toolchain"]
fn independent_steps_preserve_artifacts_in_any_order(order: [&str; 3]) -> Result<()> {
    let mut test = TestEnvironment::new()?;
    let krate = Crate::local(&fixture("hello-world"));
    test.environment.release(&krate).run(|build| {
        let mut html = Vec::new();
        let mut json = Vec::new();
        // Repeat both artifact-producing steps to check that each invocation
        // receives a unique destination, including outside build_docs().
        for mode in order.into_iter().chain(["html", "json"]) {
            match mode {
                "html" => html.push(
                    build
                        .build_documentation(docsrs_metadata::HOST_TARGET)?
                        .into_inner(),
                ),
                "json" => json.push(
                    build
                        .build_rustdoc_json(docsrs_metadata::HOST_TARGET)?
                        .into_inner(),
                ),
                "coverage" => assert!(
                    build
                        .build_coverage(docsrs_metadata::HOST_TARGET)?
                        .into_inner()
                        .is_some()
                ),
                _ => unreachable!(),
            }
        }
        assert_ne!(html[0].path(), html[1].path());
        assert_ne!(json[0].path(), json[1].path());
        for output in html {
            assert!(output.path().join("hello_world/index.html").is_file());
        }
        for output in json {
            assert!(output.format_version().is_ok());
        }
        Ok(())
    })?;
    Ok(())
}

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

    let duration = build.duration();
    let release = build.into_inner();
    let target = release.default_target();
    let steps_duration = target.coverage().duration()
        + target.rustdoc_json().duration()
        + target.documentation().duration();

    assert!(target.duration() >= steps_duration);
    assert!(duration >= release.targets().map(|target| target.duration()).sum());
    assert!(release.build_succeeded());
    assert!(release.has_docs());
    assert!(release.default_target().rustdoc_json().is_ok());
    assert!(release.default_target().coverage().is_ok());
    assert!(
        release
            .default_target()
            .coverage()
            .as_inner()
            .is_ok_and(|coverage| coverage.is_some())
    );
    assert!(
        release
            .default_target()
            .rustdoc_json()
            .as_inner()
            .expect("successful JSON build has an output")
            .format_version()
            .is_ok()
    );
    let html = target.documentation().as_inner().unwrap().path().to_owned();
    let json = target.rustdoc_json().as_inner().unwrap().path().to_owned();
    let original_json = fs::read(&json)?;
    let library = release.cargo_metadata().root().library_name().unwrap();
    drop(release);
    assert!(html.join(library).join("index.html").is_file());
    assert_eq!(fs::read(json)?, original_json);
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
        .run(|build| Ok(build.build_docs()))?
        .into_inner();

    assert!(!release.has_docs());
    assert!(!release.cargo_metadata().root().is_library());
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
        .run(|build| Ok(build.build_docs()))?
        .into_inner();

    assert!(release.build_succeeded());
    assert!(release.has_docs());
    assert!(release.default_target().coverage().is_ok());
    assert!(release.default_target().rustdoc_json().is_ok());
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
        .run(|build| Ok(build.build_docs()))?
        .into_inner();
    assert!(release.build_succeeded());
    Ok(())
}

#[test]
#[ignore = "requires Docker and a Rust toolchain"]
fn builds_coverage_and_json_for_crates_with_examples() -> Result<()> {
    let mut test = TestEnvironment::new()?;
    let release = build_local(&mut test.environment, "with-examples")?.into_inner();

    assert!(release.build_succeeded());
    assert!(release.default_target().coverage().is_ok());
    assert!(
        release
            .default_target()
            .coverage()
            .as_inner()
            .is_ok_and(|coverage| coverage.is_some())
    );
    assert!(release.default_target().rustdoc_json().is_ok());
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
        .run(|build| Ok(build.build_docs()))?
        .into_inner();

    assert!(release.build_succeeded());
    assert!(release.default_target().coverage().is_ok());
    assert!(release.default_target().rustdoc_json().is_ok());
    Ok(())
}

#[test]
#[ignore = "requires Docker and a Rust toolchain"]
fn collects_compiler_metrics() -> Result<()> {
    let workspace = test_workspace();
    let metrics = tempfile::tempdir()?;
    let mut environment = BuildEnvironment::builder(workspace.as_path())
        .wait_for_workspace_lock(true)
        .fast_init(true)
        .validate_host_resources(false)
        .sandbox_image(docs_rs_rustwide::testing::test_sandbox_image())
        .compiler_metrics_collection_path(metrics.path())
        .build()?;

    let release = build_local(&mut environment, "hello-world")?.into_inner();
    let metric_files = release.default_target().compiler_metrics().unwrap();
    assert_eq!(metric_files.len(), 1);
    let _: serde_json::Value = serde_json::from_slice(&fs::read(&metric_files[0])?)?;
    Ok(())
}

#[test_case(CpuLimit::Quota(2.0.try_into().unwrap()))]
#[test_case(CpuLimit::Cores((1..=2).try_into().unwrap()))]
#[ignore = "requires Docker and a Rust toolchain"]
fn builds_with_cpu_restrictions(cpu_limit: CpuLimit) -> Result<()> {
    let workspace = test_workspace();
    let mut environment = BuildEnvironment::builder(workspace.as_path())
        .wait_for_workspace_lock(true)
        .fast_init(true)
        .validate_host_resources(false)
        .sandbox_image(docs_rs_rustwide::testing::test_sandbox_image())
        .cpu_limit(cpu_limit)
        .build()?;
    assert!(
        build_local(&mut environment, "hello-world")?
            .into_inner()
            .build_succeeded()
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

    let fetched = test.environment.release(&krate).fetch()?;
    fetched.copy_source_to(destination.path())?;

    let release = fetched.run(|build| Ok(build.build_docs()))?.into_inner();

    assert!(destination.path().join("src/main.rs").is_file());
    assert!(!release.build_succeeded());
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
        .run(|build| Ok(build.build_docs()))?
        .into_inner();

    assert!(
        release
            .cargo_metadata()
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
        .cargo_metadata()
        .root()
        .features
        .keys()
        .map(|f| f.to_string())
        .collect();

    assert_eq!(features, ["alloc", "default", "optional_regex", "std"]);
    assert!(!features.contains(&"regex".to_string()));
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
        .run(|build| Ok(build.build_docs()))
        .err()
        .context("the published crate unexpectedly built")?;

    assert!(error.to_string().contains("Cargo.toml"));
    Ok(())
}
