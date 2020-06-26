use crates_index::Crate;
use failure::ResultExt;
use std::io::{Seek, SeekFrom, Write};

pub(crate) struct Crates {
    repo: git2::Repository,
}

impl Crates {
    pub(super) fn new(repo: git2::Repository) -> Self {
        Self { repo }
    }

    pub(crate) fn walk(&self, mut f: impl FnMut(Crate)) -> Result<(), failure::Error> {
        log::debug!("Walking crates in index");
        let tree = self
            .repo
            .find_commit(self.repo.refname_to_id("refs/remotes/origin/master")?)?
            .tree()?;

        // crates_index doesn't publicly expose their slice constructor, so need to write each blob
        // to a file before loading it as a `Crate`.
        let mut tmp = tempfile::NamedTempFile::new()?;

        let mut result = Ok(());

        tree.walk(git2::TreeWalkMode::PreOrder, |_, entry| {
            result = (|| {
                if let Some(blob) = entry.to_object(&self.repo)?.as_blob() {
                    tmp.write_all(blob.content())?;
                    if let Ok(krate) = Crate::new(tmp.path()) {
                        f(krate);
                    } else {
                        log::warn!("Not a crate {}", entry.name().unwrap());
                    }
                    tmp.as_file().set_len(0)?;
                    tmp.seek(SeekFrom::Start(0))?;
                }
                Result::<(), failure::Error>::Ok(())
            })()
            .with_context(|_| {
                format!(
                    "Loading crate details from {}",
                    entry.name().unwrap_or_default()
                )
            });
            match result {
                Ok(_) => git2::TreeWalkResult::Ok,
                Err(_) => git2::TreeWalkResult::Abort,
            }
        })?;

        Ok(result?)
    }
}
