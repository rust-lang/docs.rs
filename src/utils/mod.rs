//! Various utilities for docs.rs

pub(crate) use self::cargo_metadata::{CargoMetadata, Package as MetadataPackage};
pub(crate) use self::copy::copy_dir_all;
pub use self::daemon::{start_daemon, watch_registry};
pub(crate) use self::html::rewrite_lol;
pub use self::queue::{get_crate_priority, remove_crate_priority, set_crate_priority};
pub use self::queue_builder::queue_builder;
pub(crate) use self::rustc_version::{get_correct_docsrs_style_file, parse_rustc_version};

#[cfg(test)]
pub(crate) use self::cargo_metadata::{Dependency, Target};

mod cargo_metadata;
#[cfg(feature = "consistency_check")]
pub mod consistency;
mod copy;
pub mod daemon;
mod html;
mod queue;
pub(crate) mod queue_builder;
mod rustc_version;
use anyhow::Result;
use postgres::Client;
pub(crate) mod sized_buffer;

pub(crate) const APP_USER_AGENT: &str = concat!(
    env!("CARGO_PKG_NAME"),
    " ",
    include_str!(concat!(env!("OUT_DIR"), "/git_version"))
);

pub(crate) fn report_error(err: &anyhow::Error) {
    if std::env::var("SENTRY_DSN").is_ok() {
        sentry_anyhow::capture_anyhow(err);
    } else {
        // Debug-format for anyhow errors includes context & backtrace
        log::error!("{:?}", err);
    }
}

#[derive(strum::IntoStaticStr)]
#[strum(serialize_all = "snake_case")]
pub enum ConfigName {
    RustcVersion,
    LastSeenIndexReference,
    QueueLocked,
}

pub fn set_config(
    conn: &mut Client,
    name: ConfigName,
    value: impl Into<serde_json::Value>,
) -> anyhow::Result<()> {
    let name: &'static str = name.into();
    conn.execute(
        "INSERT INTO config (name, value) 
        VALUES ($1, $2)
        ON CONFLICT (name) DO UPDATE SET value = $2;",
        &[&name, &value.into()],
    )?;
    Ok(())
}

pub fn get_config(conn: &mut Client, name: ConfigName) -> Result<serde_json::Value> {
    let name: &'static str = name.into();
    Ok(conn
        .query_opt("SELECT value FROM config WHERE name = $1;", &[&name])?
        .map_or(serde_json::Value::Null, |row| {
            row.get::<_, serde_json::Value>("value")
        }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test::wrapper;
    use serde_json::Value;
    use test_case::test_case;

    #[test_case(ConfigName::RustcVersion, "rustc_version")]
    #[test_case(ConfigName::QueueLocked, "queue_locked")]
    #[test_case(ConfigName::LastSeenIndexReference, "last_seen_index_reference")]
    fn test_configname_variants(variant: ConfigName, expected: &'static str) {
        let name: &'static str = variant.into();
        assert_eq!(name, expected);
    }

    #[test]
    fn test_get_config_empty() {
        wrapper(|env| {
            let mut conn = env.db().conn();
            conn.execute("DELETE FROM config", &[])?;

            assert_eq!(
                get_config(&mut conn, ConfigName::RustcVersion)?,
                Value::Null
            );
            Ok(())
        });
    }

    #[test]
    fn test_set_and_get_config_() {
        wrapper(|env| {
            let mut conn = env.db().conn();
            conn.execute("DELETE FROM config", &[])?;

            assert_eq!(
                get_config(&mut conn, ConfigName::RustcVersion)?,
                Value::Null
            );

            set_config(
                &mut conn,
                ConfigName::RustcVersion,
                Value::String("some value".into()),
            )?;
            assert_eq!(
                get_config(&mut conn, ConfigName::RustcVersion)?,
                Value::String("some value".into())
            );
            Ok(())
        });
    }
}
