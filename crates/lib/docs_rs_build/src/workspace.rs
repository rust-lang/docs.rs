use anyhow::Result;
use rustwide::{
    Workspace, WorkspaceBuilder,
    cmd::{CommandError, SandboxImage},
};
use std::path::PathBuf;

/// User agent used when the docs.rs build environment accesses remote services.
pub const DOCS_RS_USER_AGENT: &str = "docs.rs builder (https://github.com/rust-lang/docs.rs)";

/// Configuration for the rustwide workspace used by a docs.rs build.
#[derive(Clone, Debug)]
pub struct WorkspaceConfig {
    /// Persistent directory containing rustwide state and caches.
    pub path: PathBuf,
    /// Whether the build driver itself is running in a container.
    pub running_inside_docker: bool,
    /// Optional override for rustwide's default sandbox image.
    pub sandbox_image: Option<String>,
    /// Prefer initialization speed over runtime performance.
    pub fast_init: bool,
}

impl WorkspaceConfig {
    /// Create a workspace configuration using rustwide's default sandbox image.
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self {
            path: path.into(),
            running_inside_docker: false,
            sandbox_image: None,
            fast_init: false,
        }
    }

    /// Initialize the configured rustwide workspace.
    pub fn init(&self) -> Result<Workspace> {
        let mut builder = WorkspaceBuilder::new(&self.path, DOCS_RS_USER_AGENT)
            .running_inside_docker(self.running_inside_docker)
            .fast_init(self.fast_init);

        if let Some(image_name) = &self.sandbox_image {
            let image = resolve_image(image_name)?;
            builder = builder.sandbox_image(image);
        }

        Ok(builder.init()?)
    }
}

fn resolve_image(name: &str) -> Result<SandboxImage> {
    match SandboxImage::local(name) {
        Ok(image) => Ok(image),
        Err(CommandError::SandboxImageMissing(_)) => Ok(SandboxImage::remote(name)?),
        Err(error) => Err(error.into()),
    }
}
