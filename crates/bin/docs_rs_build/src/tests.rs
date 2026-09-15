use super::{artifacts, report};
use anyhow::Result;
use docs_rs_rustwide::{
    BuildEnvironment, BuildStepError, SandboxImageSource, StepReport, testing::test_workspace_path,
};
use rustwide::{Crate, cmd::CommandError};
use std::{fs, path::Path, time::Duration};

#[test]
#[ignore = "requires Docker and a Rust toolchain"]
fn exports_artifacts_and_applies_exit_policy() -> Result<()> {
    docs_rs_rustwide::logging::init(false);
    let workspace = test_workspace_path();
    let mut environment = BuildEnvironment::builder(workspace.as_path())
        .wait_for_workspace_lock(true)
        .fast_init(true)
        .validate_host_resources(false)
        .sandbox_image(SandboxImageSource::linux_micro())
        .build()?;
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../lib/docs_rs_rustwide/tests/fixtures/hello-world");
    let krate = Crate::local(&fixture);
    let (mut result, additional) = environment
        .release(&krate)
        .run(|build| {
            let result = build.build_docs();
            let mut additional = build.build_target(&result.default_target.target).run();
            additional.is_default = false;
            Ok((result, additional))
        })?
        .into_inner();

    assert!(report::build_succeeded(&result, false));
    assert!(report::build_succeeded(&result, true));
    let output = tempfile::tempdir()?;
    let first = artifacts::save(&result, output.path())?;
    let second = artifacts::save(&result, output.path())?;
    assert_ne!(first, second);
    let target = result.default_target.target.clone();
    let temporary_html = result
        .default_target
        .documentation()
        .unwrap()
        .path()
        .to_owned();
    let temporary_json = result
        .default_target
        .rustdoc_json()
        .unwrap()
        .path()
        .to_owned();
    let original_json = fs::read(&temporary_json)?;
    let library = result.cargo_metadata.root().library_name().unwrap();

    result.other_targets.push(additional);
    assert!(report::build_succeeded(&result, true));
    result.other_targets[0].documentation = Err(StepReport {
        value: BuildStepError::Prepare(anyhow::anyhow!("additional target unavailable")),
        duration: Duration::ZERO,
        log: None,
    });
    assert!(report::build_succeeded(&result, false));
    assert!(!report::build_succeeded(&result, true));
    result.other_targets.clear();

    let successful_json = std::mem::replace(
        &mut result.default_target.rustdoc_json,
        Err(StepReport {
            value: BuildStepError::Output(anyhow::anyhow!("placeholder")),
            duration: Duration::ZERO,
            log: None,
        }),
    );
    // Auxiliary failures of every kind are fatal only in strict mode.
    for error in [
        BuildStepError::Prepare(anyhow::anyhow!("target unavailable")),
        BuildStepError::Command(CommandError::SandboxOOM),
        BuildStepError::Output(anyhow::anyhow!("invalid JSON")),
    ] {
        result.default_target.rustdoc_json = Err(StepReport {
            value: error,
            duration: Duration::ZERO,
            log: Some("diagnostics".into()),
        });
        assert!(report::build_succeeded(&result, false));
        assert!(!report::build_succeeded(&result, true));
    }
    result.default_target.rustdoc_json = successful_json;
    result.default_target.coverage = Err(StepReport {
        value: BuildStepError::Prepare(anyhow::anyhow!("coverage preparation failed")),
        duration: Duration::ZERO,
        log: None,
    });
    assert!(report::build_succeeded(&result, false));
    assert!(!report::build_succeeded(&result, true));

    // A successful command without the crate's docs must still fail.
    fs::remove_dir_all(temporary_html.join(&library))?;
    assert!(!report::build_succeeded(&result, false));
    assert!(!report::build_succeeded(&result, true));
    drop(result);
    assert!(!temporary_html.exists());
    assert!(!temporary_json.exists());
    for directory in [first, second] {
        assert!(
            directory
                .join(&target)
                .join("html")
                .join(&library)
                .join("index.html")
                .is_file()
        );
        assert_eq!(
            fs::read(directory.join(&target).join("rustdoc.json"))?,
            original_json
        );
    }
    Ok(())
}
