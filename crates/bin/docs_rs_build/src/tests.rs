use super::report;
use anyhow::Result;
use docs_rs_rustwide::{
    BuildEnvironment, BuildStepError, SandboxImageSource, StepReport, testing::test_workspace_path,
};
use rustwide::{Crate, cmd::CommandError};
use std::{fs, path::Path, time::Duration};

#[test]
#[ignore = "requires Docker and a Rust toolchain"]
fn retains_artifacts_and_applies_exit_policy() -> Result<()> {
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
            let mut additional = build.build_target(result.default_target.target()).run();
            additional.is_default = false;
            Ok((result, additional))
        })?
        .into_inner();

    assert!(report::build_succeeded(&result, false));
    assert!(report::build_succeeded(&result, true));
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
    result.other_targets[0].documentation = Err(StepReport::new(
        BuildStepError::Prepare(anyhow::anyhow!("additional target unavailable")),
        Duration::ZERO,
        None,
    ));
    assert!(report::build_succeeded(&result, false));
    assert!(!report::build_succeeded(&result, true));
    result.other_targets.clear();

    let successful_json = std::mem::replace(
        &mut result.default_target.rustdoc_json,
        Err(StepReport::new(
            BuildStepError::Output(anyhow::anyhow!("placeholder")),
            Duration::ZERO,
            None,
        )),
    );
    // Auxiliary failures of every kind are fatal only in strict mode.
    for error in [
        BuildStepError::Prepare(anyhow::anyhow!("target unavailable")),
        BuildStepError::Command(CommandError::SandboxOOM),
        BuildStepError::Output(anyhow::anyhow!("invalid JSON")),
    ] {
        result.default_target.rustdoc_json = Err(StepReport::new(
            error,
            Duration::ZERO,
            Some("diagnostics".into()),
        ));
        assert!(report::build_succeeded(&result, false));
        assert!(!report::build_succeeded(&result, true));
    }
    result.default_target.rustdoc_json = successful_json;
    result.default_target.coverage = Err(StepReport::new(
        BuildStepError::Prepare(anyhow::anyhow!("coverage preparation failed")),
        Duration::ZERO,
        None,
    ));
    assert!(report::build_succeeded(&result, false));
    assert!(!report::build_succeeded(&result, true));

    // A successful command without the crate's docs must still fail.
    fs::remove_dir_all(temporary_html.join(&library))?;
    assert!(!report::build_succeeded(&result, false));
    assert!(!report::build_succeeded(&result, true));
    drop(result);
    assert!(temporary_html.is_dir());
    assert_eq!(fs::read(&temporary_json)?, original_json);
    Ok(())
}
