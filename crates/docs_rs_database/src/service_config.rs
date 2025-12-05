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
