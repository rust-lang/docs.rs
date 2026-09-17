use super::*;
use crate::{SandboxImageSource, StepResultExt as _};
use std::os::unix::fs::PermissionsExt as _;
use test_case::test_case;

fn environment() -> Result<BuildEnvironment> {
    crate::logging::init(false);
    let workspace = crate::testing::test_workspace_path();
    BuildEnvironment::builder(workspace.as_path())
        .wait_for_workspace_lock(true)
        .fast_init(true)
        .validate_host_resources(false)
        .sandbox_image(SandboxImageSource::local_or_remote(
            crate::SANDBOX_IMAGE_LINUX_MICRO,
        ))
        .build()
}

fn fixture() -> rustwide::Crate {
    rustwide::Crate::local(
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/hello-world"),
    )
}

// Installed after Rustwide's preparation, which normally removes Cargo config.
// Fail only the selected rustdoc mode, leaving metadata and other modes real.
fn install_rustdoc_wrapper(build: &ReleaseBuild<'_, '_>, failure: &str) -> Result<()> {
    let source = build.build.host_source_dir();
    let script = source.join("test-rustdoc.sh");
    fs::write(
        &script,
        format!(
            r#"#!/bin/sh
case "$*" in
    *--show-coverage*) mode=coverage ;;
    *"--output-format json"*) mode=json ;;
    *--emit=html*) mode=html ;;
    *) mode=other ;;
esac
if [ "$mode" != other ]; then
    if [ "$DOCS_RS" != 1 ]; then echo "missing DOCS_RS" >&2; exit 1; fi
    echo "$mode" >> /opt/rustwide/target/rustdoc-calls.txt
    if [ "$mode" = '{failure}' ]; then
        echo "forced $mode failure" >&2
        exit 1
    fi
fi
exec /opt/rustwide/cargo-home/bin/rustdoc +nightly "$@"
"#
        ),
    )?;
    fs::set_permissions(&script, fs::Permissions::from_mode(0o755))?;
    fs::create_dir_all(source.join(".cargo"))?;
    fs::write(
        source.join(".cargo/config.toml"),
        "[build]\nrustdoc = \"/opt/rustwide/workdir/test-rustdoc.sh\"\n",
    )?;
    Ok(())
}

#[test_case("coverage")]
#[test_case("json")]
#[ignore = "requires Docker and a Rust toolchain"]
fn auxiliary_failure_does_not_prevent_html(mode: &str) -> Result<()> {
    let mut environment = environment()?;
    let release = environment
        .release(&fixture())
        .run(|build| {
            install_rustdoc_wrapper(&build, mode)?;
            Ok(build.build_docs())
        })?
        .into_inner();
    assert!(release.has_docs());
    let target = release.default_target();
    let failure = if mode == "coverage" {
        assert!(target.rustdoc_json().is_ok());
        target.coverage().as_ref().unwrap_err()
    } else {
        assert!(target.coverage().is_ok());
        target.rustdoc_json().as_ref().unwrap_err()
    };
    assert!(matches!(failure.value(), BuildStepError::Command(_)));
    assert!(
        failure
            .log()
            .unwrap()
            .contains(&format!("forced {mode} failure"))
    );
    assert!(target.regenerate_lockfile().is_none());
    Ok(())
}

#[test]
#[ignore = "requires Docker and a Rust toolchain"]
fn unavailable_additional_target_preserves_other_results() -> Result<()> {
    let mut environment = environment()?;
    let release = environment
        .release(&fixture())
        .run(|mut build| {
            build.docsrs_metadata = r#"
[package]
name = "hello-world"
[package.metadata.docs.rs]
additional-targets = ["docsrs-invalid-target", "aarch64-unknown-linux-gnu"]
"#
            .parse()?;
            Ok(build.build_docs())
        })?
        .into_inner();
    assert!(release.has_docs());
    assert_eq!(release.other_targets().len(), 2);
    let failed = release
        .other_targets()
        .iter()
        .find(|t| t.target() == "docsrs-invalid-target")
        .unwrap();
    assert!(matches!(
        failed.documentation().as_ref().unwrap_err().value(),
        BuildStepError::Prepare(_)
    ));
    assert!(failed.documentation().log().is_some());
    assert!(failed.regenerate_lockfile().is_none());
    let successful = release
        .other_targets()
        .iter()
        .find(|t| t.target() == "aarch64-unknown-linux-gnu")
        .unwrap();
    assert!(successful.has_docs("hello_world"));
    assert!(successful.rustdoc_json().is_ok());
    Ok(())
}

#[test]
#[ignore = "requires Docker and a Rust toolchain"]
fn failed_default_html_skips_additional_targets() -> Result<()> {
    let mut environment = environment()?;
    let release = environment
        .release(&fixture())
        .run(|mut build| {
            build.docsrs_metadata = r#"
[package]
name = "hello-world"
[package.metadata.docs.rs]
additional-targets = ["docsrs-invalid-target"]
"#
            .parse()?;
            install_rustdoc_wrapper(&build, "html")?;
            Ok(build.build_docs())
        })?
        .into_inner();
    assert!(!release.has_docs());
    assert!(release.default_target().documentation().is_err());
    assert!(release.default_target().rustdoc_json().is_ok());
    assert!(release.other_targets().is_empty());
    Ok(())
}

#[test_case(true, 2; "retry once")]
#[test_case(false, 1; "retry disabled")]
#[ignore = "requires Docker and a Rust toolchain"]
fn html_command_retry_is_bounded(retry: bool, expected_attempts: usize) -> Result<()> {
    let mut environment = environment()?;
    environment.release(&fixture()).run(|build| {
        install_rustdoc_wrapper(&build, "html")?;
        let target = build
            .build_target(HOST_TARGET)
            .retry_without_lockfile(retry)
            .run();
        assert!(matches!(
            target.documentation().as_ref().unwrap_err().value(),
            BuildStepError::Command(_)
        ));
        assert_eq!(target.regenerate_lockfile().is_some(), retry);
        if retry {
            assert!(target.regenerate_lockfile().unwrap().is_ok());
        }
        let calls = fs::read_to_string(build.build.host_target_dir().join("rustdoc-calls.txt"))?;
        assert_eq!(
            calls.lines().filter(|&mode| mode == "html").count(),
            expected_attempts
        );
        Ok(())
    })?;
    Ok(())
}

#[test_case("docsrs-invalid-target", false; "preparation failure")]
#[test_case(HOST_TARGET, true; "output failure")]
#[ignore = "requires Docker and a Rust toolchain"]
fn non_command_failure_does_not_regenerate(target: &str, output_failure: bool) -> Result<()> {
    let mut environment = environment()?;
    environment.release(&fixture()).run(|build| {
        if output_failure {
            // Cargo succeeds, but preserving its output must fail because the
            // artifact parent is a file rather than a directory.
            let target_dir = build.build.host_target_dir();
            fs::write(target_dir.parent().unwrap().join("tmp"), "blocked")?;
        }
        let result = build
            .build_target(target)
            .retry_without_lockfile(true)
            .run();
        let error = result.documentation().as_ref().unwrap_err().value();
        if output_failure {
            assert!(matches!(error, BuildStepError::Output(_)));
        } else {
            assert!(matches!(error, BuildStepError::Prepare(_)));
        }
        assert!(result.regenerate_lockfile().is_none());
        Ok(())
    })?;
    Ok(())
}

#[test]
#[ignore = "requires Docker and a Rust toolchain"]
fn metrics_collection_failure_is_nonfatal() -> Result<()> {
    crate::logging::init(false);
    let workspace = crate::testing::test_workspace_path();
    let temporary = tempfile::tempdir()?;
    let destination = temporary.path().join("not-a-directory");
    fs::write(&destination, "blocked")?;
    let mut environment = BuildEnvironment::builder(workspace.as_path())
        .wait_for_workspace_lock(true)
        .fast_init(true)
        .validate_host_resources(false)
        .sandbox_image(SandboxImageSource::local_or_remote(
            crate::SANDBOX_IMAGE_LINUX_MICRO,
        ))
        .compiler_metrics_collection_path(destination.as_path())
        .build()?;
    environment.release(&fixture()).run(|build| {
        let release = build.build_docs();
        assert!(release.has_docs());
        assert!(release.default_target().compiler_metrics().is_none());
        let source = build.compiler_metrics_dir().unwrap();
        assert!(
            fs::read_dir(source)?.next().is_some(),
            "failed collection must retain metrics"
        );
        Ok(())
    })?;
    Ok(())
}
