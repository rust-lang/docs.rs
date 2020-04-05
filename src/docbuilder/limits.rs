use crate::error::Result;
use postgres::Connection;
use std::collections::BTreeMap;
use std::time::Duration;

#[derive(Debug, Clone, PartialEq, Eq)]
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

    pub(crate) fn for_website(&self) -> BTreeMap<String, String> {
        let mut res = BTreeMap::new();
        res.insert("Available RAM".into(), SIZE_SCALE(self.memory));
        res.insert(
            "Maximum rustdoc execution time".into(),
            TIME_SCALE(self.timeout.as_secs() as usize),
        );
        res.insert(
            "Maximum size of a build log".into(),
            SIZE_SCALE(self.max_log_size),
        );
        if self.networking {
            res.insert("Network access".into(), "allowed".into());
        } else {
            res.insert("Network access".into(), "blocked".into());
        }
        res.insert(
            "Maximum number of build targets".into(),
            self.targets.to_string(),
        );
        res
    }
}

const TIME_SCALE: fn(usize) -> String = |v| scale(v, 60, &["seconds", "minutes", "hours"]);
const SIZE_SCALE: fn(usize) -> String = |v| scale(v, 1024, &["bytes", "KB", "MB", "GB"]);

fn scale(value: usize, interval: usize, labels: &[&str]) -> String {
    let (mut value, interval) = (value as f64, interval as f64);
    let mut chosen_label = &labels[0];
    for label in &labels[1..] {
        if value / interval >= 1.0 {
            chosen_label = label;
            value /= interval;
        } else {
            break;
        }
    }
    // 2.x
    let mut value = format!("{:.1}", value);
    // 2.0 -> 2
    if value.ends_with(".0") {
        value.truncate(value.len() - 2);
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

    #[test]
    fn display_limits() {
        let limits = Limits {
            memory: 102_400,
            timeout: Duration::from_secs(300),
            targets: 1,
            ..Limits::default()
        };
        let display = limits.for_website();
        assert_eq!(display.get("Network access"), Some(&"blocked".into()));
        assert_eq!(
            display.get("Maximum size of a build log"),
            Some(&"100 KB".into())
        );
        assert_eq!(
            display.get("Maximum number of build targets"),
            Some(&limits.targets.to_string())
        );
        assert_eq!(
            display.get("Maximum rustdoc execution time"),
            Some(&"5 minutes".into())
        );
        assert_eq!(display.get("Available RAM"), Some(&"100 KB".into()));
    }

    #[test]
    fn scale_limits() {
        // time
        assert_eq!(TIME_SCALE(300), "5 minutes");
        assert_eq!(TIME_SCALE(1), "1 seconds");
        assert_eq!(TIME_SCALE(7200), "2 hours");

        // size
        assert_eq!(SIZE_SCALE(1), "1 bytes");
        assert_eq!(SIZE_SCALE(100), "100 bytes");
        assert_eq!(SIZE_SCALE(1024), "1 KB");
        assert_eq!(SIZE_SCALE(10240), "10 KB");
        assert_eq!(SIZE_SCALE(1_048_576), "1 MB");
        assert_eq!(SIZE_SCALE(10_485_760), "10 MB");
        assert_eq!(SIZE_SCALE(1_073_741_824), "1 GB");
        assert_eq!(SIZE_SCALE(10_737_418_240), "10 GB");
        assert_eq!(SIZE_SCALE(std::u32::MAX as usize), "4 GB");

        // fractional sizes
        assert_eq!(TIME_SCALE(90), "1.5 minutes");
        assert_eq!(TIME_SCALE(5400), "1.5 hours");

        assert_eq!(SIZE_SCALE(1_288_490_189), "1.2 GB");
        assert_eq!(SIZE_SCALE(3_758_096_384), "3.5 GB");
        assert_eq!(SIZE_SCALE(1_048_051_712), "999.5 MB");
    }
}
