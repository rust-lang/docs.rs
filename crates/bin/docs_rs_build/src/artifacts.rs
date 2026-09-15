use anyhow::{Context as _, Result};
use docs_rs_rustwide::{ReleaseBuildResult, utils::copy_dir_all};
use std::{
    fs,
    path::{Path, PathBuf},
};

/// Export artifacts before dropping the library result and its temporary files.
pub(crate) fn save(result: &ReleaseBuildResult, workspace: &Path) -> Result<PathBuf> {
    let parent = workspace.join("artifacts");
    fs::create_dir_all(&parent).context("creating artifact directory")?;
    let directory = tempfile::Builder::new()
        .prefix("build-")
        .tempdir_in(parent)?;
    for target in result.targets() {
        let destination = directory.path().join(&target.target);
        fs::create_dir_all(&destination)?;
        if let Ok(html) = target.documentation()
            && html.path().is_dir()
        {
            copy_dir_all(html, destination.join("html"), |_| {})
                .with_context(|| format!("exporting HTML for {}", target.target))?;
        }
        if let Ok(json) = target.rustdoc_json() {
            fs::copy(json, destination.join("rustdoc.json"))
                .with_context(|| format!("exporting JSON for {}", target.target))?;
        }
    }
    Ok(directory.keep())
}
