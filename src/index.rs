use crate::error::Result;
use crate::utils::report_error;
use anyhow::Context;
use crates_index_diff::gix;
use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::AtomicBool;

pub struct Index {
    path: PathBuf,
    repository_url: Option<String>,
}

impl Index {
    pub fn from_url(path: PathBuf, url: String) -> Result<Self> {
        crates_index_diff::Index::from_path_or_cloned_with_options(
            &path,
            gix::progress::Discard,
            &AtomicBool::default(),
            crates_index_diff::index::CloneOptions { url: url.clone() },
        )
        .map(|_| ())
        .context("initialising registry index repository")?;

        Ok(Self {
            path,
            repository_url: Some(url),
        })
    }

    pub fn new(path: PathBuf) -> Result<Self> {
        // This initializes the repository, then closes it afterwards to avoid leaking file descriptors.
        // See https://github.com/rust-lang/docs.rs/pull/847
        crates_index_diff::Index::from_path_or_cloned(&path)
            .map(|_| ())
            .context("initialising registry index repository")?;
        Ok(Self {
            path,
            repository_url: None,
        })
    }

    pub fn diff(&self) -> Result<crates_index_diff::Index> {
        let options = self
            .repository_url
            .clone()
            .map(|url| crates_index_diff::index::CloneOptions { url })
            .unwrap_or_default();
        let diff = crates_index_diff::Index::from_path_or_cloned_with_options(
            &self.path,
            gix::progress::Discard,
            &AtomicBool::default(),
            options,
        )
        .context("re-opening registry index for diff")?;
        Ok(diff)
    }

    pub(crate) fn crates(&self) -> Result<crates_index::GitIndex> {
        tracing::debug!("Opening with `crates_index`");
        // crates_index requires the repo url to match the existing origin or it tries to reinitialize the repo
        let repo_url = self
            .repository_url
            .as_deref()
            .unwrap_or("https://github.com/rust-lang/crates.io-index");
        let mut index = crates_index::GitIndex::with_path(&self.path, repo_url)?;
        index.update()?;
        Ok(index)
    }

    pub fn run_git_gc(&self) {
        let gc = Command::new("git")
            .arg("-C")
            .arg(&self.path)
            .args(["gc", "--auto"])
            .output()
            .with_context(|| format!("failed to run `git gc --auto`\npath: {:#?}", &self.path));

        if let Err(err) = gc {
            report_error(&err);
        }
    }

    pub fn repository_url(&self) -> Option<&str> {
        self.repository_url.as_deref()
    }
}
