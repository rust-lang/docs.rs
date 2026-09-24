use super::report;
use anyhow::Result;
use docs_rs_rustwide::{BuildEnvironment, StepResultExt as _, testing::test_workspace_path};
use rustwide::Crate;
use std::{fs, path::Path};

#[test]
#[ignore = "requires Docker and a Rust toolchain"]
fn retains_artifacts_and_applies_exit_policy() -> Result<()> {
    docs_rs_rustwide::logging::init(false);
    let workspace = test_workspace_path();
    let mut environment = BuildEnvironment::builder(workspace.as_path())
        .wait_for_workspace_lock(true)
        .fast_init(true)
        .validate_host_resources(false)
        .sandbox_image(docs_rs_rustwide::testing::test_sandbox_image())
        .build()?;
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../lib/docs_rs_rustwide/tests/fixtures/additional-targets");
    let krate = Crate::local(&fixture);
    let result = environment
        .release(&krate)
        .run(|build| Ok(build.build_docs()))?
        .into_inner();

    assert!(report::build_succeeded(&result, false));
    assert!(report::build_succeeded(&result, true));
    let temporary_html = result
        .default_target()
        .documentation()
        .as_inner()
        .unwrap()
        .path()
        .to_owned();
    let temporary_json = result
        .default_target()
        .rustdoc_json()
        .as_inner()
        .unwrap()
        .path()
        .to_owned();
    let original_json = fs::read(&temporary_json)?;
    let library = result.cargo_metadata().root().library_name().unwrap();

    assert!(!result.other_targets().is_empty());
    assert!(report::build_succeeded(&result, true));
    fs::remove_dir_all(
        result.other_targets()[0]
            .documentation()
            .as_inner()
            .unwrap()
            .path(),
    )?;
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
