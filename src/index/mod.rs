use std::sync::atomic::AtomicBool;
use std::{path::PathBuf, process::Command};

use anyhow::Context;
use crates_index_diff::gix;
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
fn load_config(repo: &gix::Repository) -> Result<IndexConfig> {
    let file = repo
        .rev_parse_single("refs/remotes/origin/master:config.json")
        .with_context(|| anyhow::anyhow!("registry index missing ./config.json in root"))?
        .object()?;

    let config = serde_json::from_slice(&file.data)?;
    Ok(config)
}

impl Index {
    pub fn from_url(path: PathBuf, url: String) -> Result<Self> {
        let diff = crates_index_diff::Index::from_path_or_cloned_with_options(
            &path,
            gix::progress::Discard,
            &AtomicBool::default(),
            crates_index_diff::index::CloneOptions { url: url.clone() },
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

    #[cfg(feature = "consistency_check")]
    pub(crate) fn crates(&self) -> Result<crates_index::Index> {
        tracing::debug!("Opening with `crates_index`");
        // crates_index requires the repo url to match the existing origin or it tries to reinitialize the repo
        let repo_url = self
            .repository_url
            .as_deref()
            .unwrap_or("https://github.com/rust-lang/crates.io-index");
        let mut index = crates_index::Index::with_path(&self.path, repo_url)?;
        index.update()?;
        Ok(index)
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
