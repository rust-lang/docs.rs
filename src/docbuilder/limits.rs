use crate::db::Overrides;
use crate::error::Result;
use postgres::Client;
use serde::Serialize;
use std::time::Duration;

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
            targets: crate::DEFAULT_MAX_TARGETS,
            networking: false,
            max_log_size: 100 * 1024, // 100 KB
        }
    }
}

impl Limits {
    pub(crate) fn for_crate(conn: &mut Client, name: &str) -> Result<Self> {
        let default = Self::default();
        let overrides = Overrides::for_crate(conn, name)?.unwrap_or_default();
        Ok(Self {
            memory: overrides.memory.unwrap_or(default.memory),
            targets: overrides
                .targets
                .or(overrides.timeout.map(|_| 1))
                .unwrap_or(default.targets),
            timeout: overrides.timeout.unwrap_or(default.timeout),
            networking: default.networking,
            max_log_size: default.max_log_size,
        })
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
            let hexponent = Limits::for_crate(&mut db.conn(), krate)?;
            assert_eq!(hexponent, Limits::default());

            Overrides::save(
                &mut db.conn(),
                krate,
                Overrides {
                    targets: Some(15),
                    ..Overrides::default()
                },
            )?;
            // limits work if crate has limits set
            let hexponent = Limits::for_crate(&mut db.conn(), krate)?;
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
            Overrides::save(
                &mut db.conn(),
                krate,
                Overrides {
                    memory: Some(limits.memory),
                    targets: Some(limits.targets),
                    timeout: Some(limits.timeout),
                },
            )?;
            assert_eq!(limits, Limits::for_crate(&mut db.conn(), krate)?);
            Ok(())
        });
    }

    #[test]
    fn targets_default_to_one_with_timeout() {
        wrapper(|env| {
            let db = env.db();
            let krate = "hexponent";
            Overrides::save(
                &mut db.conn(),
                krate,
                Overrides {
                    timeout: Some(Duration::from_secs(20 * 60)),
                    ..Overrides::default()
                },
            )?;
            let limits = Limits::for_crate(&mut db.conn(), krate)?;
            assert_eq!(limits.targets, 1);

            Ok(())
        });
    }
}
