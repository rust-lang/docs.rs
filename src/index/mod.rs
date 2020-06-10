use std::path::{Path, PathBuf};

use log::info;
use url::Url;

use self::api::Api;
use crate::error::Result;

pub(crate) mod api;

pub(crate) struct Index {
    diff: crates_index_diff::Index,
    path: PathBuf,
    api: Option<Api>,
}

#[derive(serde::Deserialize, Clone)]
#[serde(rename_all = "kebab-case")]
struct IndexConfig {
    dl: String,
    #[serde(default)]
    api: Option<Url>,
    #[serde(default)]
    allowed_registries: Vec<String>,
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
        .ok_or_else(|| failure::format_err!("registry index missing config"))?;
    let config = serde_json::from_slice(repo.find_blob(file.id())?.content())?;
    Ok(config)
}

impl Index {
    pub(crate) fn new(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref().to_owned();
        let diff = crates_index_diff::Index::from_path_or_cloned(&path)?;
        let config = load_config(diff.repository())?;
        let api = if let Some(api_base) = &config.api {
            Some(Api::new(api_base)?)
        } else {
            info!("Cannot load registry data as index is missing an api base url");
            None
        };
        Ok(Self { diff, path, api })
    }

    pub(crate) fn diff(&self) -> &crates_index_diff::Index {
        &self.diff
    }

    pub(crate) fn api(&self) -> Option<&Api> {
        self.api.as_ref()
    }
}

impl Clone for Index {
    fn clone(&self) -> Self {
        Self::new(&self.path).expect("we already loaded this registry successfully once")
    }
}
