#![allow(dead_code)]

use anyhow::Result;
use docs_rs_logging::MessageOnlyLogTracer;
use docs_rs_rustwide::BuildEnvironment;
pub use docs_rs_rustwide::testing::{TestWorkspace, test_sandbox_image};
use rustwide::{BuildResult, Crate};
use std::{
    path::{Path, PathBuf},
    sync::Once,
};
use tracing::level_filters::LevelFilter;

static INIT_LOGGING: Once = Once::new();

pub fn init_logging() {
    INIT_LOGGING.call_once(|| {
        tracing_subscriber::fmt()
            .with_max_level(LevelFilter::DEBUG)
            .with_test_writer()
            .try_init()
            .expect("failed to initialize test tracing");
        rustwide::logging::init_with(MessageOnlyLogTracer);
    });
}

pub struct TestEnvironment {
    _workspace: TestWorkspace,
    pub environment: BuildEnvironment,
}

impl TestEnvironment {
    pub fn new() -> Result<Self> {
        Self::new_inner(false)
    }

    pub fn with_default_targets() -> Result<Self> {
        Self::new_inner(true)
    }

    fn new_inner(include_default_targets: bool) -> Result<Self> {
        init_logging();
        let workspace = TestWorkspace::acquire()?;
        let environment = BuildEnvironment::builder(workspace.path())
            .fast_init(true)
            .validate_host_resources(false)
            .sandbox_image(test_sandbox_image())
            .include_default_targets(include_default_targets)
            .build()?;
        Ok(Self {
            _workspace: workspace,
            environment,
        })
    }
}

pub fn test_workspace() -> Result<TestWorkspace> {
    init_logging();
    TestWorkspace::acquire()
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
