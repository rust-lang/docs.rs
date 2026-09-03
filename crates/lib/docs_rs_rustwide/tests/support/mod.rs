#![allow(dead_code)]

use anyhow::Result;
use docs_rs_rustwide::{BuildEnvironment, SandboxImageSource};
use rustwide::{BuildResult, Crate};
use std::{
    fs::{self, File, OpenOptions},
    path::{Path, PathBuf},
    sync::Once,
};
use tracing::level_filters::LevelFilter;
use tracing_log::LogTracer;

const TEST_SANDBOX_IMAGE: &str = "ghcr.io/rust-lang/crates-build-env/linux-micro";
const TEST_WORKSPACE_ENV: &str = "DOCSRS_RUSTWIDE_WORKSPACE";
static INIT_LOGGING: Once = Once::new();

pub fn init_logging() {
    INIT_LOGGING.call_once(|| {
        tracing_subscriber::fmt()
            .with_max_level(LevelFilter::DEBUG)
            .with_test_writer()
            .try_init()
            .expect("failed to initialize test tracing");
        rustwide::logging::init_with(LogTracer::new());
    });
}

pub struct TestEnvironment {
    _workspace: TestWorkspace,
    pub environment: BuildEnvironment,
}

impl TestEnvironment {
    pub fn new() -> Result<Self> {
        init_logging();
        let workspace = test_workspace()?;
        let environment = BuildEnvironment::builder(workspace.path())
            .fast_init(true)
            .validate_host_resources(false)
            .sandbox_image(test_sandbox_image())
            .build()?;
        Ok(Self {
            _workspace: workspace,
            environment,
        })
    }
}

/// A persistent rustwide workspace locked for exclusive use by one test.
///
/// Reusing the workspace avoids reinstalling rustup and the configured
/// toolchain for every ignored integration test. The lock prevents tests in
/// this crate from concurrently purging or updating the shared workspace.
pub struct TestWorkspace {
    path: PathBuf,
    _lock: File,
}

impl TestWorkspace {
    pub fn path(&self) -> &Path {
        &self.path
    }
}

pub fn test_workspace() -> Result<TestWorkspace> {
    init_logging();
    let path = std::env::var_os(TEST_WORKSPACE_ENV)
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .ancestors()
                .nth(3)
                .expect("docs_rs_rustwide must be inside the workspace")
                .join(".workspace")
        });
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }

    let mut lock_path = path.as_os_str().to_os_string();
    lock_path.push(".test-lock");
    let lock = OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .open(PathBuf::from(lock_path))?;
    tracing::debug!(workspace = %path.display(), "waiting for test workspace lock");
    lock.lock()?;
    tracing::debug!(workspace = %path.display(), "acquired test workspace lock");

    Ok(TestWorkspace { path, _lock: lock })
}

pub fn test_sandbox_image() -> SandboxImageSource {
    SandboxImageSource::LocalOrRemote(TEST_SANDBOX_IMAGE.into())
}

pub fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
}

pub fn build_local(
    environment: &mut BuildEnvironment,
    fixture_name: &str,
) -> Result<BuildResult<docs_rs_rustwide::ReleaseBuildResult>> {
    init_logging();
    let fixture = fixture(fixture_name);
    let krate = Crate::local(&fixture);
    environment.release(&krate).run(|build| build.build_docs())
}
