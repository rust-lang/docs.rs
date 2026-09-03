#![allow(dead_code)]

use anyhow::Result;
use docs_rs_rustwide::{BuildEnvironment, SandboxImageSource};
use rustwide::{BuildResult, Crate};
use std::{
    path::{Path, PathBuf},
    sync::Once,
};
use tempfile::TempDir;
use tracing::level_filters::LevelFilter;
use tracing_log::LogTracer;

const TEST_SANDBOX_IMAGE: &str = "ghcr.io/rust-lang/crates-build-env/linux-micro";
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
    _workspace: TempDir,
    pub environment: BuildEnvironment,
}

impl TestEnvironment {
    pub fn new() -> Result<Self> {
        init_logging();
        let workspace = tempfile::tempdir()?;
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
