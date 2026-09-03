#![allow(dead_code)]

use anyhow::Result;
use docs_rs_build::{BuildEnvironment, SandboxImageSource};
use rustwide::{BuildResult, Crate};
use std::path::{Path, PathBuf};
use tempfile::TempDir;

const TEST_SANDBOX_IMAGE: &str = "ghcr.io/rust-lang/crates-build-env/linux-micro";

pub struct TestEnvironment {
    _workspace: TempDir,
    pub environment: BuildEnvironment,
}

impl TestEnvironment {
    pub fn new() -> Result<Self> {
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
) -> Result<BuildResult<docs_rs_build::ReleaseBuildResult>> {
    let fixture = fixture(fixture_name);
    let krate = Crate::local(&fixture);
    environment.release(&krate).run(|build| build.build_docs())
}
