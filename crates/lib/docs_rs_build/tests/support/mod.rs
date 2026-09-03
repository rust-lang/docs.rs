#![allow(dead_code)]

use anyhow::Result;
use docs_rs_build::BuildEnvironment;
use rustwide::{BuildResult, Crate};
use std::path::{Path, PathBuf};
use tempfile::TempDir;

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
            .build()?;
        Ok(Self {
            _workspace: workspace,
            environment,
        })
    }
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
