use crate::error::Result;
use postgres::Connection;
use serde::Serialize;
use std::{time::Duration};

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct Limits {
    memory: usize,
    targets: usize,
    timeout: Duration,
    networking: bool,
    max_log_size: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            memory: 3 * 1024 * 1024 * 1024,        // 3 GB
            timeout: Duration::from_secs(15 * 60), // 15 minutes
            targets: 10,
            networking: false,
            max_log_size: 100 * 1024, // 100 KB
        }
    }
}

impl Limits {
    pub(crate) fn for_crate(conn: &Connection, name: &str) -> Result<Self> {
        let mut limits = Self::default();

        let res = conn.query(
            "SELECT * FROM sandbox_overrides WHERE crate_name = $1;",
            &[&name],
        )?;
        if !res.is_empty() {
            let row = res.get(0);
            if let Some(memory) = row.get::<_, Option<i64>>("max_memory_bytes") {
                limits.memory = memory as usize;
            }
            if let Some(timeout) = row.get::<_, Option<i32>>("timeout_seconds") {
                limits.timeout = Duration::from_secs(timeout as u64);
            }
            if let Some(targets) = row.get::<_, Option<i32>>("max_targets") {
                limits.targets = targets as usize;
            }
        }

        Ok(limits)
    }

    pub(crate) fn memory(&self) -> usize {
        self.memory
    }

    pub(crate) fn timeout(&self) -> Duration {
        self.timeout
    }

    pub(crate) fn networking(&self) -> bool {
        self.networking
    }

    pub(crate) fn max_log_size(&self) -> usize {
        self.max_log_size
    }

    pub(crate) fn targets(&self) -> usize {
        self.targets
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use crate::test::*;

    #[test]
    fn retrieve_limits() {
        wrapper(|env| {
            let db = env.db();

            let krate = "hexponent";
            // limits work if no crate has limits set
            let hexponent = Limits::for_crate(&db.conn(), krate)?;
            assert_eq!(hexponent, Limits::default());

            db.conn().query(
                "INSERT INTO sandbox_overrides (crate_name, max_targets) VALUES ($1, 15)",
                &[&krate],
            )?;
            // limits work if crate has limits set
            let hexponent = Limits::for_crate(&db.conn(), krate)?;
            assert_eq!(
                hexponent,
                Limits {
                    targets: 15,
                    ..Limits::default()
                }
            );

            // all limits work
            let krate = "regex";
            let limits = Limits {
                memory: 100_000,
                timeout: Duration::from_secs(300),
                targets: 1,
                ..Limits::default()
            };
            db.conn().query(
                "INSERT INTO sandbox_overrides (crate_name, max_memory_bytes, timeout_seconds, max_targets)
                 VALUES ($1, $2, $3, $4)",
                &[&krate, &(limits.memory as i64), &(limits.timeout.as_secs() as i32), &(limits.targets as i32)]
            )?;
            assert_eq!(limits, Limits::for_crate(&db.conn(), krate)?);
            Ok(())
        });
    }
}
