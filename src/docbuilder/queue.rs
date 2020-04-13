//! Updates crates.io index and builds new packages

use super::{DocBuilder, RustwideBuilder};
use crate::db::connect_db;
use crate::error::Result;
use crate::utils::{add_crate_to_queue, get_crate_priority};
use crates_index_diff::{ChangeKind, Index};

impl DocBuilder {
    /// Updates crates.io-index repository and adds new crates into build queue.
    /// Returns size of queue
    pub fn get_new_crates(&mut self) -> Result<usize> {
        let conn = connect_db()?;
        let index = Index::from_path_or_cloned(&self.options.crates_io_index_path)?;
        let mut changes = index.fetch_changes()?;
        let mut add_count: usize = 0;

        // I belive this will fix ordering of queue if we get more than one crate from changes
        changes.reverse();

        for krate in changes.iter().filter(|k| k.kind != ChangeKind::Yanked) {
            let priority = get_crate_priority(&conn, &krate.name)?;
            add_crate_to_queue(&conn, &krate.name, &krate.version, priority).ok();

            debug!("{}-{} added into build queue", krate.name, krate.version);
            add_count += 1;
        }

        Ok(add_count)
    }

    pub fn get_queue_count(&self) -> Result<i64> {
        let conn = connect_db()?;

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
        let conn = connect_db()?;

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

        match builder.build_package(self, &name, &version, None) {
            Ok(_) => {
                let _ = conn.execute("DELETE FROM queue WHERE id = $1", &[&id]);
                crate::web::metrics::TOTAL_BUILDS.inc();
            }
            Err(e) => {
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
                    "Failed to build package {}-{} from queue: {}",
                    name, version, e
                )
            }
        }

        Ok(true)
    }
}

#[cfg(test)]
mod test {
    use crate::{DocBuilder, DocBuilderOptions};
    use std::path::PathBuf;

    #[test]
    #[ignore]
    fn test_get_new_crates() {
        let _ = env_logger::try_init();
        let options = DocBuilderOptions::from_prefix(PathBuf::from("../cratesfyi-prefix"));
        let mut docbuilder = DocBuilder::new(options);
        let res = docbuilder.get_new_crates();
        if res.is_err() {
            error!("{:?}", res);
        }
        assert!(res.is_ok());
    }
}
