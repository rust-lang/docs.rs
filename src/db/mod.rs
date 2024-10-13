//! Database operations
use anyhow::Result;
use sqlx::migrate::{Migrate, Migrator};

pub use self::add_package::update_latest_version_id;
pub(crate) use self::add_package::{
    add_doc_coverage, add_package_into_database, finish_build, initialize_build, initialize_crate,
    initialize_release, update_build_with_error,
};
pub use self::{
    add_package::{update_build_status, update_crate_data_in_database},
    delete::{delete_crate, delete_version},
    file::{add_path_into_database, add_path_into_remote_archive},
    overrides::Overrides,
    pool::{AsyncPoolClient, Pool, PoolError},
};

mod add_package;
pub mod blacklist;
pub mod delete;
pub(crate) mod file;
pub(crate) mod mimes;
mod overrides;
mod pool;
pub(crate) mod types;

static MIGRATOR: Migrator = sqlx::migrate!();

pub async fn migrate(conn: &mut sqlx::PgConnection, target: Option<i64>) -> Result<()> {
    conn.ensure_migrations_table().await?;

    // `database_versions` is the table that tracked the old `schemamama` migrations.
    // If we find the table, and it contains records, we insert a fake record
    // into the `_sqlx_migrations` table so the big initial migration isn't executed.
    if sqlx::query(
        "SELECT table_name
         FROM information_schema.tables
         WHERE table_schema = 'public' AND table_name = 'database_versions'",
    )
    .fetch_optional(&mut *conn)
    .await?
    .is_some()
    {
        let max_version: Option<i64> =
            sqlx::query_scalar("SELECT max(version) FROM database_versions")
                .fetch_one(&mut *conn)
                .await?;

        if max_version != Some(39) {
            anyhow::bail!(
                "database_versions table has unexpected version: {:?}",
                max_version
            );
        }

        sqlx::query(
            "INSERT INTO _sqlx_migrations ( version, description, success, checksum, execution_time )
             VALUES ( $1, $2, TRUE, $3, -1 )",
        )
        // the next two parameters relate to the filename of the initial migration file
        .bind(20231021111635i64)
        .bind("initial")
        // this is the hash of the initial migration file, as sqlx requires it.
        // if the initial migration file changes, this has to be updated with the new value,
        // easiest to get from the `_sqlx_migrations` table when the migration was normally
        // executed.
        .bind(hex::decode("df802e0ec416063caadd1c06b13348cd885583c44962998886b929d5fe6ef3b70575d5101c5eb31daa989721df08d806").unwrap())
        .execute(&mut *conn)
        .await?;

        sqlx::query("DROP TABLE database_versions")
            .execute(&mut *conn)
            .await?;
    }

    // when we find records
    if let Some(target) = target {
        MIGRATOR.undo(conn, target).await?;
    } else {
        MIGRATOR.run(conn).await?;
    }
    Ok(())
}
