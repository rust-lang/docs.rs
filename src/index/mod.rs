use std::{path::PathBuf, process::Command};

use anyhow::Context;
use url::Url;

use self::api::Api;
use crate::error::Result;
use crate::utils::report_error;

pub(crate) mod api;

pub struct Index {
    path: PathBuf,
    api: Api,
    repository_url: Option<String>,
}

#[derive(Debug, serde::Deserialize, Clone)]
#[serde(rename_all = "kebab-case")]
struct IndexConfig {
    #[serde(default)]
    api: Option<Url>,
}

/// Inspects the given repository to find the config as specified in [RFC 2141][], assumes that the
/// repository has a remote called `origin` and that the branch `master` exists on it.
///
/// [RFC 2141]: https://rust-lang.github.io/rfcs/2141-alternative-registries.html
fn load_config(repo: &git2::Repository) -> Result<IndexConfig> {
    let tree = repo
        .find_commit(repo.refname_to_id("refs/remotes/origin/master")?)?
        .tree()?;
    let file = tree
        .get_name("config.json")
        .with_context(|| anyhow::anyhow!("registry index missing config"))?;
    let config = serde_json::from_slice(repo.find_blob(file.id())?.content())?;
    Ok(config)
}

impl Index {
    pub fn from_url(path: PathBuf, repository_url: String) -> Result<Self> {
        let url = repository_url.clone();
        let diff = crates_index_diff::Index::from_path_or_cloned_with_options(
            &path,
            crates_index_diff::CloneOptions {
                repository_url,
                ..Default::default()
            },
        )
        .context("initialising registry index repository")?;

        let config = load_config(diff.repository()).context("loading registry config")?;
        let api = Api::new(config.api).context("initialising registry api client")?;
        Ok(Self {
            path,
            api,
            repository_url: Some(url),
        })
    }

    pub fn new(path: PathBuf) -> Result<Self> {
        // This initializes the repository, then closes it afterwards to avoid leaking file descriptors.
        // See https://github.com/rust-lang/docs.rs/pull/847
        let diff = crates_index_diff::Index::from_path_or_cloned(&path)
            .context("initialising registry index repository")?;
        let config = load_config(diff.repository()).context("loading registry config")?;
        let api = Api::new(config.api).context("initialising registry api client")?;
        Ok(Self {
            path,
            api,
            repository_url: None,
        })
    }

    pub(crate) fn diff(&self) -> Result<crates_index_diff::Index> {
        let options = self
            .repository_url
            .clone()
            .map(|repository_url| crates_index_diff::CloneOptions {
                repository_url,
                ..Default::default()
            })
            .unwrap_or_default();
        let diff = crates_index_diff::Index::from_path_or_cloned_with_options(&self.path, options)
            .context("re-opening registry index for diff")?;
        Ok(diff)
    }

    #[cfg(feature = "consistency_check")]
    pub(crate) fn crates(&self) -> Result<crates_index::Index> {
        // First ensure the index is up to date, peeking will pull the latest changes without
        // affecting anything else.
        log::debug!("Updating index");
        self.diff()?.peek_changes()?;
        log::debug!("Opening with `crates_index`");
        // crates_index requires the repo url to match the existing origin or it tries to reinitialize the repo
        let repo_url = self
            .repository_url
            .as_deref()
            .unwrap_or("https://github.com/rust-lang/crates.io-index");
        crates_index::Index::with_path(&self.path, repo_url).map_err(Into::into)
    }

    pub fn api(&self) -> &Api {
        &self.api
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
