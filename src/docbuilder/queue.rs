//! Updates registry index and builds new packages

use super::{DocBuilder, RustwideBuilder};
use crate::error::Result;
use crate::utils::get_crate_priority;
use crates_index_diff::ChangeKind;
use log::{debug, error};

impl DocBuilder {
    /// Updates registry index repository and adds new crates into build queue.
    /// Returns the number of crates added
    pub fn get_new_crates(&mut self) -> Result<usize> {
        let mut conn = self.db.get()?;
        let diff = self.index.diff()?;
        let (mut changes, oid) = diff.peek_changes()?;
        let mut crates_added = 0;

        // I believe this will fix ordering of queue if we get more than one crate from changes
        changes.reverse();

        for krate in &changes {
            match krate.kind {
                ChangeKind::Yanked => {
                    let res = conn.execute(
                        "
                        UPDATE releases
                            SET yanked = TRUE
                        FROM crates
                        WHERE crates.id = releases.crate_id
                            AND name = $1
                            AND version = $2
                        ",
                        &[&krate.name, &krate.version],
                    );
                    match res {
                        Ok(_) => debug!("{}-{} yanked", krate.name, krate.version),
                        Err(err) => error!(
                            "error while setting {}-{} to yanked: {}",
                            krate.name, krate.version, err
                        ),
                    }
                }

                ChangeKind::Added => {
                    let priority = get_crate_priority(&mut conn, &krate.name)?;

                    match self
                        .build_queue
                        .add_crate(&krate.name, &krate.version, priority)
                    {
                        Ok(()) => {
                            debug!("{}-{} added into build queue", krate.name, krate.version);
                            crates_added += 1;
                        }
                        Err(err) => error!(
                            "failed adding {}-{} into build queue: {}",
                            krate.name, krate.version, err
                        ),
                    }
                }
            }
        }

        diff.set_last_seen_reference(oid)?;

        Ok(crates_added)
    }

    /// Builds the top package from the queue. Returns whether there was a package in the queue.
    ///
    /// Note that this will return `Ok(true)` even if the package failed to build.
    pub(crate) fn build_next_queue_package(
        &mut self,
        builder: &mut RustwideBuilder,
    ) -> Result<bool> {
        let mut processed = false;
        let queue = self.build_queue.clone();
        queue.process_next_crate(|krate| {
            processed = true;

            builder.build_package(self, &krate.name, &krate.version, None)?;
            Ok(())
        })?;

        Ok(processed)
    }

    pub fn run_git_gc(&self) {
        self.index.run_git_gc();
    }
}
