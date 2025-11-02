use crate::{
    error::Result,
    utils::{report_error, run_blocking},
};
use anyhow::Context as _;
use crates_index_diff::{Change, gix, index::diff::Order};
use std::{
    path::{Path, PathBuf},
    sync::{Arc, Mutex, atomic::AtomicBool},
};
use tokio::process::Command;

const THREAD_NAME: &str = "crates-index-diff";

/// async-friendly wrapper around `crates_index_diff::Index`
pub struct Index {
    path: PathBuf,
    repository_url: Option<String>,
    // NOTE: we can use a sync mutex here, because we're only locking it
    // inside handle_in_thread calls, so the mutex lock won't ever be held
    // across await-points.
    #[allow(clippy::disallowed_types)]
    index: Arc<Mutex<crates_index_diff::Index>>,
}

impl Index {
    pub async fn from_url(
        path: impl AsRef<Path>,
        repository_url: Option<impl AsRef<str>>,
    ) -> Result<Self> {
        let path = path.as_ref().to_path_buf();
        let repository_url = repository_url.map(|url| url.as_ref().to_owned());

        let clone_options = repository_url
            .as_ref()
            .map(|url| crates_index_diff::index::CloneOptions { url: url.clone() })
            .unwrap_or_default();

        let index = run_blocking(THREAD_NAME, {
            let path = path.clone();
            move || {
                Ok(Arc::new(Mutex::new(
                    #[allow(clippy::disallowed_types)]
                    crates_index_diff::Index::from_path_or_cloned_with_options(
                        &path,
                        gix::progress::Discard,
                        &AtomicBool::default(),
                        clone_options,
                    )
                    .context("initialising registry index repository")?,
                )))
            }
        })
        .await?;

        Ok(Self {
            index,
            path,
            repository_url,
        })
    }

    pub async fn new(path: impl AsRef<Path>) -> Result<Self> {
        Self::from_url(path, None::<&str>).await
    }

    pub async fn run_git_gc(&self) {
        let gc = Command::new("git")
            .arg("-C")
            .arg(&self.path)
            .args(["gc", "--auto"])
            .output()
            .await
            .with_context(|| format!("failed to run `git gc --auto`\npath: {:#?}", &self.path));

        if let Err(err) = gc {
            report_error(&err);
        }
    }

    async fn peek_changes_with_order(
        &self,
        order: Order,
    ) -> Result<(Vec<Change>, gix::hash::ObjectId)> {
        let index = self.index.clone();
        run_blocking(THREAD_NAME, move || {
            let index = index.lock().unwrap();
            index
                .peek_changes_with_options(gix::progress::Discard, &AtomicBool::default(), order)
                .map_err(Into::into)
        })
        .await
    }

    pub async fn peek_changes(&self) -> Result<(Vec<Change>, gix::hash::ObjectId)> {
        self.peek_changes_with_order(Order::ImplementationDefined)
            .await
    }

    pub async fn peek_changes_ordered(&self) -> Result<(Vec<Change>, gix::hash::ObjectId)> {
        self.peek_changes_with_order(Order::AsInCratesIndex).await
    }

    pub async fn set_last_seen_reference(&self, to: gix::hash::ObjectId) -> Result<()> {
        let index = self.index.clone();
        run_blocking(THREAD_NAME, move || {
            let index = index.lock().unwrap();
            index.set_last_seen_reference(to).map_err(Into::into)
        })
        .await
    }

    pub async fn latest_commit_reference(&self) -> Result<gix::ObjectId> {
        let (_, oid) = self.peek_changes().await?;
        Ok(oid)
    }

    pub fn repository_url(&self) -> Option<&str> {
        self.repository_url.as_deref()
    }
}
