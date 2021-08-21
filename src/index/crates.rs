use anyhow::Context;
use crates_index::Crate;
pub(crate) struct Crates {
    repo: git2::Repository,
}

impl Crates {
    pub(super) fn new(repo: git2::Repository) -> Self {
        Self { repo }
    }

    pub(crate) fn walk(&self, mut f: impl FnMut(Crate)) -> Result<(), anyhow::Error> {
        log::debug!("Walking crates in index");
        let tree = self
            .repo
            .find_commit(self.repo.refname_to_id("refs/remotes/origin/master")?)?
            .tree()?;

        let mut result = Ok(());

        tree.walk(git2::TreeWalkMode::PreOrder, |_, entry| {
            result = (|| {
                if let Some(blob) = entry.to_object(&self.repo)?.as_blob() {
                    if let Ok(krate) = Crate::from_slice(blob.content()) {
                        f(krate);
                    } else {
                        log::warn!("not a crate '{}'", entry.name().unwrap());
                    }
                }
                Result::<(), anyhow::Error>::Ok(())
            })()
            .with_context(|| {
                format!(
                    "loading crate details from '{}'",
                    entry.name().unwrap_or("<unknown>")
                )
            });
            match result {
                Ok(_) => git2::TreeWalkResult::Ok,
                Err(_) => git2::TreeWalkResult::Abort,
            }
        })?;

        result
    }
}
