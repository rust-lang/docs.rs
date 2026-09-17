use super::*;

#[test]
fn maintenance_schedule_and_selection_changes() {
    let interval = Duration::from_secs(60);
    let mut toolchain = ManagedToolchain::new(Toolchain::dist("nightly"), interval);
    let now = Instant::now();
    assert!(toolchain.update_due(now));
    toolchain.mark_updated(now);
    assert!(!toolchain.update_due(now + interval - Duration::from_nanos(1)));
    assert!(toolchain.update_due(now + interval));
    assert!(!toolchain.select(Toolchain::dist("nightly")));
    assert!(!toolchain.update_due(now));
    assert!(toolchain.select(Toolchain::dist("stable")));
    assert!(toolchain.update_due(now));
    assert_eq!(toolchain.get(), &Toolchain::dist("stable"));
}

#[test]
fn zero_interval_always_checks_for_updates() {
    let mut toolchain = ManagedToolchain::new(Toolchain::dist("nightly"), Duration::ZERO);
    let now = Instant::now();
    toolchain.mark_updated(now);
    assert!(toolchain.update_due(now));
}

#[test]
fn parses_rustc_resource_version() {
    assert_eq!(
        parse_rustc_version("rustc 1.10.0-nightly (57ef01513 2016-05-23)").unwrap(),
        "20160523-1.10.0-nightly-57ef01513"
    );
}

#[test]
fn creates_ci_rustc_resource_version() {
    assert_eq!(
        parse_rustc_version(&ci_rustc_version("0123456789abcdef")).unwrap(),
        "29991229-1.9999.0-nightly-0123456789abcdef"
    );
}

fn environment() -> Result<crate::BuildEnvironment> {
    crate::logging::init(false);
    crate::BuildEnvironment::builder(crate::testing::test_workspace_path().as_path())
        .toolchain(Toolchain::dist("nightly"))
        .wait_for_workspace_lock(true)
        .fast_init(true)
        .validate_host_resources(false)
        .sandbox_image(crate::testing::test_sandbox_image())
        .build()
}

#[test]
#[ignore = "requires Docker, network access, and a Rust toolchain"]
fn readiness_reuses_installed_toolchain() -> Result<()> {
    let environment = environment()?;
    let toolchain = ManagedToolchain::new(
        environment.toolchain().clone(),
        DEFAULT_TOOLCHAIN_UPDATE_INTERVAL,
    );
    let workspace = environment.workspace();
    assert!(toolchain.is_toolchain_installed(workspace)?);
    assert!(
        !toolchain.ensure_ready(workspace)?,
        "must reuse the existing installation"
    );
    assert!(
        !toolchain.ensure_ready(workspace)?,
        "readiness must remain idempotent"
    );
    Ok(())
}

#[test]
#[ignore = "requires Docker, network access, and a Rust toolchain"]
fn readiness_installs_required_targets() -> Result<()> {
    let environment = environment()?;
    let toolchain = ManagedToolchain::new(
        environment.toolchain().clone(),
        DEFAULT_TOOLCHAIN_UPDATE_INTERVAL,
    );
    let workspace = environment.workspace();
    // Remove a managed non-host target to exercise restoration, rather than merely
    // asserting the setup performed by BuildEnvironment.
    let target = DEFAULT_TARGETS
        .iter()
        .copied()
        .find(|target| *target != HOST_TARGET)
        .unwrap();
    toolchain.get().remove_target(workspace, target)?;
    assert!(
        !toolchain
            .get()
            .installed_targets(workspace)?
            .iter()
            .any(|installed| installed == target)
    );
    let ready = toolchain.ensure_ready(workspace);
    let installed = toolchain.get().installed_targets(workspace);
    // Attempt restoration even when readiness fails, since tests share this workspace.
    let restore = toolchain.ensure_target_installed(workspace, target);
    assert!(!ready?);
    restore?;
    let installed = installed?;
    for target in ManagedToolchain::managed_toolchain_targets() {
        assert!(
            installed.contains(&target),
            "required target {target} is missing"
        );
    }
    Ok(())
}

#[test]
#[ignore = "requires Docker, network access, and a Rust toolchain"]
fn detects_installed_compiler_version() -> Result<()> {
    let environment = environment()?;
    let toolchain = ManagedToolchain::new(
        environment.toolchain().clone(),
        DEFAULT_TOOLCHAIN_UPDATE_INTERVAL,
    );
    let workspace = environment.workspace();
    let version = toolchain.rustc_version(workspace)?;
    let verbose = Command::new(workspace, toolchain.get().rustc())
        .arg("--version")
        .arg("--verbose")
        .run_capture()?;
    assert_eq!(verbose.stdout_lines().first(), Some(&version));
    assert!(version.starts_with("rustc "));
    assert_eq!(
        toolchain.resource_suffix(workspace)?,
        format!("-{}", parse_rustc_version(&version)?)
    );
    Ok(())
}
