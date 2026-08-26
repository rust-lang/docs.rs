use anyhow::Result;
use sqlx::migrate::{Migrate, Migrator};

pub static MIGRATOR: Migrator = sqlx::migrate!(); // defaults to "./migrations"

pub async fn migrate(conn: &mut sqlx::PgConnection, target: Option<i64>) -> Result<()> {
    conn.ensure_migrations_table(&MIGRATOR.table_name).await?;

    // when we find records
    if let Some(target) = target {
        MIGRATOR.undo(conn, target).await?;
    } else {
        MIGRATOR.run(conn).await?;
    }
    Ok(())
}
