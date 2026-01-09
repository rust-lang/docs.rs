use anyhow::Result;
use serde::{Serialize, de::DeserializeOwned};

#[derive(strum::IntoStaticStr)]
#[strum(serialize_all = "snake_case")]
pub enum ConfigName {
    RustcVersion,
    LastSeenIndexReference,
    QueueLocked,
    Toolchain,
}

pub async fn set_config(
    conn: &mut sqlx::PgConnection,
    name: ConfigName,
    value: impl Serialize,
) -> anyhow::Result<()> {
    let name: &'static str = name.into();
    sqlx::query!(
        "INSERT INTO config (name, value)
         VALUES ($1, $2)
         ON CONFLICT (name) DO UPDATE SET value = $2;",
        name,
        &serde_json::to_value(value)?,
    )
    .execute(conn)
    .await?;
    Ok(())
}

pub async fn get_config<T>(conn: &mut sqlx::PgConnection, name: ConfigName) -> Result<Option<T>>
where
    T: DeserializeOwned,
{
    let name: &'static str = name.into();
    Ok(
        match sqlx::query!("SELECT value FROM config WHERE name = $1;", name)
            .fetch_optional(conn)
            .await?
        {
            Some(row) => serde_json::from_value(row.value)?,
            None => None,
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Config, testing::TestDatabase};
    use docs_rs_config::AppConfig as _;
    use docs_rs_opentelemetry::testing::TestMetrics;
    use serde_json::Value;
    use test_case::test_case;

    #[test_case(ConfigName::RustcVersion, "rustc_version")]
    #[test_case(ConfigName::QueueLocked, "queue_locked")]
    #[test_case(ConfigName::LastSeenIndexReference, "last_seen_index_reference")]
    fn test_configname_variants(variant: ConfigName, expected: &'static str) {
        let name: &'static str = variant.into();
        assert_eq!(name, expected);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_get_config_empty() -> anyhow::Result<()> {
        let test_metrics = TestMetrics::new();
        let db = TestDatabase::new(&Config::test_config()?, test_metrics.provider()).await?;

        let mut conn = db.async_conn().await?;
        sqlx::query!("DELETE FROM config")
            .execute(&mut *conn)
            .await?;

        assert!(
            get_config::<String>(&mut conn, ConfigName::RustcVersion)
                .await?
                .is_none()
        );
        Ok(())
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_set_and_get_config_() -> anyhow::Result<()> {
        let test_metrics = TestMetrics::new();
        let db = TestDatabase::new(&Config::test_config()?, test_metrics.provider()).await?;

        let mut conn = db.async_conn().await?;
        sqlx::query!("DELETE FROM config")
            .execute(&mut *conn)
            .await?;

        assert!(
            get_config::<String>(&mut conn, ConfigName::RustcVersion)
                .await?
                .is_none()
        );

        set_config(
            &mut conn,
            ConfigName::RustcVersion,
            Value::String("some value".into()),
        )
        .await?;
        assert_eq!(
            get_config(&mut conn, ConfigName::RustcVersion).await?,
            Some("some value".to_string())
        );
        Ok(())
    }
}
