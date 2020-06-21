//! Updates registry index and builds new packages

use super::{DocBuilder, RustwideBuilder};
use crate::error::Result;
use crate::utils::{add_crate_to_queue, get_crate_priority};
use crates_index_diff::ChangeKind;
use log::{debug, error};

impl DocBuilder {
    /// Updates registry index repository and adds new crates into build queue.
    /// Returns the number of crates added
    pub fn get_new_crates(&mut self) -> Result<usize> {
        let conn = self.db.get()?;
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
                    let priority = get_crate_priority(&conn, &krate.name)?;

                    match add_crate_to_queue(&conn, &krate.name, &krate.version, priority) {
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

    pub fn get_queue_count(&self) -> Result<i64> {
        let conn = self.db.get()?;

        Ok(conn
            .query("SELECT COUNT(*) FROM queue WHERE attempt < 5", &[])?
            .get(0)
            .get(0))
    }

    /// Builds the top package from the queue. Returns whether the queue was empty.
    pub(crate) fn build_next_queue_package(
        &mut self,
        builder: &mut RustwideBuilder,
    ) -> Result<bool> {
        // This is in a nested scope to drop the connection before build_package is called,
        // otherwise the borrow checker will complain.
        let (id, name, version) = {
            let conn = self.db.get()?;

            let query = conn.query(
                "SELECT id, name, version
                                         FROM queue
                                         WHERE attempt < 5
                                         ORDER BY priority ASC, attempt ASC, id ASC
                                         LIMIT 1",
                &[],
            )?;

            if query.is_empty() {
                // nothing in the queue; bail
                return Ok(false);
            }

            let id: i32 = query.get(0).get(0);
            let name: String = query.get(0).get(1);
            let version: String = query.get(0).get(2);

            (id, name, version)
        };

        match builder.build_package(self, &name, &version, None) {
            Ok(_) => {
                let conn = self.db.get()?;

                let _ = conn.execute("DELETE FROM queue WHERE id = $1", &[&id]);
                crate::web::metrics::TOTAL_BUILDS.inc();
            }
            Err(e) => {
                let conn = self.db.get()?;

                // Increase attempt count
                let rows = conn.query(
                    "UPDATE queue SET attempt = attempt + 1 WHERE id = $1 RETURNING attempt",
                    &[&id],
                )?;
                let attempt: i32 = rows.get(0).get(0);
                if attempt >= 5 {
                    crate::web::metrics::FAILED_BUILDS.inc();
                    crate::web::metrics::TOTAL_BUILDS.inc();
                }
                error!(
                    "Failed to build package {}-{} from queue: {}\nBacktrace: {}",
                    name,
                    version,
                    e,
                    e.backtrace()
                )
            }
        }

        Ok(true)
    }
}
