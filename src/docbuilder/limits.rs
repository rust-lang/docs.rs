use error::Result;
use postgres::Connection;
use std::collections::BTreeMap;
use std::time::Duration;

#[derive(Debug, PartialEq)]
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
            memory: 3 * 1024 * 1024 * 1024,  // 3 GB
            timeout: Duration::from_secs(15 * 60),  // 15 minutes
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

    pub(crate) fn for_website(&self) -> BTreeMap<String, String> {
        let time_scale = |v| scale(v, 60, &["seconds", "minutes", "hours"]);
        let size_scale = |v| scale(v, 1024, &["bytes", "KB", "MB", "GB"]);

        let mut res = BTreeMap::new();
        res.insert("Available RAM".into(), size_scale(self.memory));
        res.insert(
            "Maximum rustdoc execution time".into(),
            time_scale(self.timeout.as_secs() as usize),
        );
        res.insert("Maximum size of a build log".into(), size_scale(self.max_log_size));
        if self.networking {
            res.insert("Network access".into(), "allowed".into());
        } else {
            res.insert("Network access".into(), "blocked".into());
        }
        res.insert("Maximum number of build targets".into(), self.targets.to_string());
        res
    }
}

fn scale(mut value: usize, interval: usize, labels: &[&str]) -> String {
    let mut chosen_label = &labels[0];
    for label in &labels[1..] {
        if (value as f64) / (interval as f64) >= 1.0 {
            chosen_label = label;
            value = value / interval;
        } else {
            break;
        }
    }
    format!("{} {}", value, chosen_label)
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

            db.conn().query("INSERT INTO sandbox_overrides (crate_name, max_targets) VALUES ($1, 15)", &[&krate])?;
            // limits work if crate has limits set
            let hexponent = Limits::for_crate(&db.conn(), krate)?;
            assert_eq!(hexponent, Limits { targets: 15, ..Limits::default() });

            // all limits work
            let krate = "regex";
            let limits = Limits {
                memory: 100_000,
                timeout: Duration::from_secs(300),
                targets: 1,
                ..Limits::default()
            };
            db.conn().query("INSERT INTO sandbox_overrides (crate_name, max_memory_bytes, timeout_seconds, max_targets)
                                    VALUES ($1, $2, $3, $4)",
                                    &[&krate, &(limits.memory as i64), &(limits.timeout.as_secs() as i32), &(limits.targets as i32)])?;
            assert_eq!(limits, Limits::for_crate(&db.conn(), krate)?);
            Ok(())
        });
    }
    #[test]
    fn display_limits() {
        let limits = Limits {
            memory: 102400,
            timeout: Duration::from_secs(300),
            targets: 1,
            ..Limits::default()
        };
        let display = limits.for_website();
        assert_eq!(display.get("Network access".into()), Some(&"blocked".into()));
        assert_eq!(display.get("Maximum size of a build log".into()), Some(&"100 KB".into()));
        assert_eq!(display.get("Maximum number of build targets".into()), Some(&limits.targets.to_string()));
        assert_eq!(display.get("Maximum rustdoc execution time".into()), Some(&"5 minutes".into()));
        assert_eq!(display.get("Available RAM".into()), Some(&"100 KB".into()));
    }
}
