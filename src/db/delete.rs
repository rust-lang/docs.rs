use crate::{
    Config,
    error::Result,
    storage::{AsyncStorage, rustdoc_archive_path, source_archive_path},
};
use anyhow::Context as _;
use docs_rs_types::{CrateId, Version};
use fn_error_context::context;
use sqlx::Connection;

use super::update_latest_version_id;

/// List of directories in docs.rs's underlying storage (either the database or S3) containing a
/// subdirectory named after the crate. Those subdirectories will be deleted.
static LIBRARY_STORAGE_PATHS_TO_DELETE: &[&str] = &["rustdoc", "rustdoc-json", "sources"];
static OTHER_STORAGE_PATHS_TO_DELETE: &[&str] = &["sources"];

#[context("error trying to delete crate {name} from database")]
pub async fn delete_crate(
    conn: &mut sqlx::PgConnection,
    storage: &AsyncStorage,
    config: &Config,
    name: &str,
) -> Result<()> {
    let Some(crate_id) = get_id(conn, name).await? else {
        return Ok(());
    };

    let is_library = delete_crate_from_database(conn, name, crate_id).await?;
    // #899
    let paths = if is_library {
        LIBRARY_STORAGE_PATHS_TO_DELETE
    } else {
        OTHER_STORAGE_PATHS_TO_DELETE
    };

    for prefix in paths {
        // delete the whole rustdoc/source folder for this crate.
        // it will include existing archives.
        let remote_folder = format!("{prefix}/{name}/");
        storage.delete_prefix(&remote_folder).await?;

        // remove existing local archive index files.
        let local_index_folder = config.local_archive_cache_path.join(&remote_folder);
        if local_index_folder.exists() {
            tokio::fs::remove_dir_all(&local_index_folder)
                .await
                .with_context(|| {
                    format!(
                        "error when trying to remove local index: {:?}",
                        &local_index_folder
                    )
                })?;
        }
    }

    Ok(())
}

#[context("error trying to delete release {name}-{version} from database")]
pub async fn delete_version(
    conn: &mut sqlx::PgConnection,
    storage: &AsyncStorage,
    config: &Config,
    name: &str,
    version: &Version,
) -> Result<()> {
    let Some(crate_id) = get_id(conn, name).await? else {
        return Ok(());
    };

    let is_library = delete_version_from_database(conn, crate_id, version).await?;
    let paths = if is_library {
        LIBRARY_STORAGE_PATHS_TO_DELETE
    } else {
        OTHER_STORAGE_PATHS_TO_DELETE
    };

    for prefix in paths {
        storage
            .delete_prefix(&format!("{prefix}/{name}/{version}/"))
            .await?;
    }

    let local_archive_cache = &config.local_archive_cache_path;
    let mut paths = vec![source_archive_path(name, version)];
    if is_library {
        paths.push(rustdoc_archive_path(name, version));
    }

    for archive_filename in paths {
        // delete remove archive and remote index
        storage.delete_prefix(&archive_filename).await?;

        // delete eventually existing local indexes
        let local_index_file = local_archive_cache.join(format!("{archive_filename}.index"));
        if local_index_file.exists() {
            tokio::fs::remove_file(&local_index_file)
                .await
                .with_context(|| {
                    format!("error when trying to remove local index: {local_index_file:?}")
                })?;
        }
    }

    Ok(())
}

async fn get_id(conn: &mut sqlx::PgConnection, name: &str) -> Result<Option<CrateId>> {
    Ok(sqlx::query_scalar!(
        r#"
        SELECT id as "id: CrateId"
        FROM crates
        WHERE normalize_crate_name(name) = normalize_crate_name($1)
        "#,
        name
    )
    .fetch_optional(&mut *conn)
    .await?)
}

// metaprogramming!
// WARNING: these must be hard-coded and NEVER user input.
const METADATA: &[(&str, &str)] = &[
    ("keyword_rels", "rid"),
    ("builds", "rid"),
    ("compression_rels", "release"),
    ("doc_coverage", "release_id"),
];

/// Returns whether this release was a library
async fn delete_version_from_database(
    conn: &mut sqlx::PgConnection,
    crate_id: CrateId,
    version: &Version,
) -> Result<bool> {
    let mut transaction = conn.begin().await?;
    for &(table, column) in METADATA {
        sqlx::query(
            format!("DELETE FROM {table} WHERE {column} IN (SELECT id FROM releases WHERE crate_id = $1 AND version = $2)").as_str())
        .bind(crate_id).bind(version).execute(&mut *transaction).await?;
    }
    let is_library: bool = sqlx::query_scalar!(
        "DELETE FROM releases WHERE crate_id = $1 AND version = $2 RETURNING is_library",
        crate_id.0,
        version as _,
    )
    .fetch_one(&mut *transaction)
    .await?
    .unwrap_or(false);

    update_latest_version_id(&mut transaction, crate_id).await?;

    transaction.commit().await?;
    Ok(is_library)
}

/// Returns whether any release in this crate was a library
async fn delete_crate_from_database(
    conn: &mut sqlx::PgConnection,
    name: &str,
    crate_id: CrateId,
) -> Result<bool> {
    let mut transaction = conn.begin().await?;

    sqlx::query!("DELETE FROM sandbox_overrides WHERE crate_name = $1", name,)
        .execute(&mut *transaction)
        .await?;

    for &(table, column) in METADATA {
        sqlx::query(
            format!(
                "DELETE FROM {table} WHERE {column} IN (SELECT id FROM releases WHERE crate_id = $1)"
            )
            .as_str()).bind(crate_id).execute(&mut *transaction).await?;
    }
    sqlx::query!("DELETE FROM owner_rels WHERE cid = $1;", crate_id.0)
        .execute(&mut *transaction)
        .await?;

    let has_library: bool = sqlx::query_scalar!(
        "SELECT
            BOOL_OR(releases.is_library) AS has_library
        FROM releases
        WHERE releases.crate_id = $1
        ",
        crate_id.0
    )
    .fetch_one(&mut *transaction)
    .await?
    .unwrap_or(false);

    sqlx::query!("DELETE FROM releases WHERE crate_id = $1;", crate_id.0)
        .execute(&mut *transaction)
        .await?;
    sqlx::query!("DELETE FROM crates WHERE id = $1;", crate_id.0)
        .execute(&mut *transaction)
        .await?;

    // Transactions automatically rollback when not committing, so if any of the previous queries
    // fail the whole transaction will be aborted.
    transaction.commit().await?;
    Ok(has_library)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::registry_api::{CrateOwner, OwnerKind};
    use crate::storage::{CompressionAlgorithm, rustdoc_json_path};
    use crate::test::{KRATE, V1, V2, async_wrapper, fake_release_that_failed_before_build};
    use docs_rs_types::ReleaseId;
    use test_case::test_case;

    async fn crate_exists(conn: &mut sqlx::PgConnection, name: &str) -> Result<bool> {
        Ok(sqlx::query!("SELECT id FROM crates WHERE name = $1;", name)
            .fetch_optional(conn)
            .await?
            .is_some())
    }

    async fn release_exists(conn: &mut sqlx::PgConnection, id: ReleaseId) -> Result<bool> {
        Ok(sqlx::query!("SELECT id FROM releases WHERE id = $1;", id.0)
            .fetch_optional(conn)
            .await?
            .is_some())
    }

    #[test]
    fn test_get_id_uses_normalization() {
        async_wrapper(|env| async move {
            env.fake_release()
                .await
                .name("Some_Package")
                .version(V1)
                .create()
                .await?;

            let mut conn = env.async_db().async_conn().await;
            assert!(get_id(&mut conn, "some-package").await.is_ok());

            Ok(())
        })
    }

    #[test_case(true)]
    #[test_case(false)]
    fn test_delete_crate(archive_storage: bool) {
        async_wrapper(|env| async move {
            let mut conn = env.async_db().async_conn().await;

            // Create fake packages in the database
            let pkg1_v1_id = env
                .fake_release()
                .await
                .name("package-1")
                .version(V1)
                .archive_storage(archive_storage)
                .create()
                .await?;
            let pkg1_v2_id = env
                .fake_release()
                .await
                .name("package-1")
                .version(V2)
                .archive_storage(archive_storage)
                .create()
                .await?;
            let pkg2_id = env
                .fake_release()
                .await
                .name("package-2")
                .version(V1)
                .archive_storage(archive_storage)
                .create()
                .await?;

            assert!(crate_exists(&mut conn, "package-1").await?);
            assert!(crate_exists(&mut conn, "package-2").await?);
            assert!(release_exists(&mut conn, pkg1_v1_id).await?);
            assert!(release_exists(&mut conn, pkg1_v2_id).await?);
            assert!(release_exists(&mut conn, pkg2_id).await?);
            for (pkg, version) in &[("package-1", V1), ("package-1", V2), ("package-2", V1)] {
                assert!(
                    env.async_storage()
                        .rustdoc_file_exists(
                            pkg,
                            version,
                            None,
                            &format!("{pkg}/index.html"),
                            archive_storage
                        )
                        .await?
                );
            }

            delete_crate(&mut conn, env.async_storage(), env.config(), "package-1").await?;

            assert!(!crate_exists(&mut conn, "package-1").await?);
            assert!(crate_exists(&mut conn, "package-2").await?);
            assert!(!release_exists(&mut conn, pkg1_v1_id).await?);
            assert!(!release_exists(&mut conn, pkg1_v2_id).await?);
            assert!(release_exists(&mut conn, pkg2_id).await?);

            // files for package 2 still exists
            assert!(
                env.async_storage()
                    .rustdoc_file_exists(
                        "package-2",
                        &V1,
                        None,
                        "package-2/index.html",
                        archive_storage
                    )
                    .await?
            );

            // files for package 1 are gone
            if archive_storage {
                assert!(
                    !env.async_storage()
                        .exists(&rustdoc_archive_path("package-1", &V1))
                        .await?
                );
                assert!(
                    !env.async_storage()
                        .exists(&rustdoc_archive_path("package-1", &V2))
                        .await?
                );
            } else {
                assert!(
                    !env.async_storage()
                        .rustdoc_file_exists(
                            "package-1",
                            &V1,
                            None,
                            "package-1/index.html",
                            archive_storage
                        )
                        .await?
                );
                assert!(
                    !env.async_storage()
                        .rustdoc_file_exists(
                            "package-1",
                            &V2,
                            None,
                            "package-1/index.html",
                            archive_storage
                        )
                        .await?
                );
            }

            Ok(())
        });
    }

    #[test_case(true)]
    #[test_case(false)]
    fn test_delete_version(archive_storage: bool) {
        async_wrapper(|env| async move {
            async fn owners(
                conn: &mut sqlx::PgConnection,
                crate_id: CrateId,
            ) -> Result<Vec<String>> {
                Ok(sqlx::query!(
                    "SELECT login FROM owners
                    INNER JOIN owner_rels ON owners.id = owner_rels.oid
                    WHERE owner_rels.cid = $1",
                    crate_id.0,
                )
                .fetch_all(conn)
                .await?
                .into_iter()
                .map(|row| row.login)
                .collect())
            }

            async fn json_exists(storage: &AsyncStorage, version: &Version) -> Result<bool> {
                storage
                    .exists(&rustdoc_json_path(
                        "a",
                        version,
                        "x86_64-unknown-linux-gnu",
                        crate::storage::RustdocJsonFormatVersion::Latest,
                        Some(CompressionAlgorithm::Zstd),
                    ))
                    .await
            }

            let mut conn = env.async_db().async_conn().await;
            let v1 = env
                .fake_release()
                .await
                .name("a")
                .version(V1)
                .archive_storage(archive_storage)
                .add_owner(CrateOwner {
                    login: "malicious actor".into(),
                    avatar: "https://example.org/malicious".into(),
                    kind: OwnerKind::User,
                })
                .create()
                .await?;
            assert!(release_exists(&mut conn, v1).await?);
            assert!(
                env.async_storage()
                    .rustdoc_file_exists("a", &V1, None, "a/index.html", archive_storage)
                    .await?
            );
            assert!(json_exists(env.async_storage(), &V1).await?);
            let crate_id = sqlx::query_scalar!(
                r#"SELECT crate_id as "crate_id: CrateId" FROM releases WHERE id = $1"#,
                v1.0
            )
            .fetch_one(&mut *conn)
            .await?;
            assert_eq!(
                owners(&mut conn, crate_id).await?,
                vec!["malicious actor".to_string()]
            );

            let v2 = env
                .fake_release()
                .await
                .name("a")
                .version(V2)
                .archive_storage(archive_storage)
                .add_owner(CrateOwner {
                    login: "Peter Rabbit".into(),
                    avatar: "https://example.org/peter".into(),
                    kind: OwnerKind::User,
                })
                .create()
                .await?;
            assert!(release_exists(&mut conn, v2).await?);
            assert!(
                env.async_storage()
                    .rustdoc_file_exists("a", &V2, None, "a/index.html", archive_storage)
                    .await?
            );
            assert!(json_exists(env.async_storage(), &V2).await?);
            assert_eq!(
                owners(&mut conn, crate_id).await?,
                vec!["Peter Rabbit".to_string()]
            );

            delete_version(&mut conn, env.async_storage(), env.config(), "a", &V1).await?;
            assert!(!release_exists(&mut conn, v1).await?);
            if archive_storage {
                // for archive storage the archive and index files
                // need to be cleaned up.
                let rustdoc_archive = rustdoc_archive_path("a", &V1);
                assert!(!env.async_storage().exists(&rustdoc_archive).await?);

                // local and remote index are gone too
                let archive_index = format!("{rustdoc_archive}.index");
                assert!(!env.async_storage().exists(&archive_index).await?);
                assert!(
                    !env.config()
                        .local_archive_cache_path
                        .join(&archive_index)
                        .exists()
                );
            } else {
                assert!(
                    !env.async_storage()
                        .rustdoc_file_exists("a", &V1, None, "a/index.html", archive_storage)
                        .await?
                );
            }
            assert!(!json_exists(env.async_storage(), &V1,).await?);

            assert!(release_exists(&mut conn, v2).await?);
            assert!(
                env.async_storage()
                    .rustdoc_file_exists("a", &V2, None, "a/index.html", archive_storage)
                    .await?
            );
            assert!(json_exists(env.async_storage(), &V2).await?);
            assert_eq!(
                owners(&mut conn, crate_id).await?,
                vec!["Peter Rabbit".to_string()]
            );

            // FIXME: remove for now until test frontend is async
            // let web = env.frontend();
            // assert_success("/a/2.0.0/a/", web)?;
            // assert_eq!(web.get("/a/1.0.0/a/").send()?.status(), 404);

            Ok(())
        })
    }

    #[test]
    fn test_delete_incomplete_version() {
        async_wrapper(|env| async move {
            let db = env.async_db();
            let mut conn = db.async_conn().await;

            let (release_id, _) =
                fake_release_that_failed_before_build(&mut conn, "a", V1, "some-error").await?;

            delete_version(&mut conn, env.async_storage(), env.config(), "a", &V1).await?;

            assert!(!release_exists(&mut conn, release_id).await?);

            Ok(())
        })
    }

    #[test]
    fn test_delete_incomplete_crate() {
        async_wrapper(|env| async move {
            let db = env.async_db();
            let mut conn = db.async_conn().await;

            let (release_id, _) =
                fake_release_that_failed_before_build(&mut conn, "a", V1, "some-error").await?;

            delete_crate(&mut conn, env.async_storage(), env.config(), "a").await?;

            assert!(!crate_exists(&mut conn, "a").await?);
            assert!(!release_exists(&mut conn, release_id).await?);

            Ok(())
        })
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_delete_missing_crate_doesnt_error() -> Result<()> {
        let env = crate::test::TestEnvironment::new().await?;

        let db = env.async_db();
        let mut conn = db.async_conn().await;

        assert!(!crate_exists(&mut conn, KRATE).await?);
        delete_crate(&mut conn, env.async_storage(), env.config(), KRATE).await?;

        assert!(!crate_exists(&mut conn, KRATE).await?);

        Ok(())
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_delete_missing_version_doesnt_error() -> Result<()> {
        let env = crate::test::TestEnvironment::new().await?;

        let db = env.async_db();
        let mut conn = db.async_conn().await;

        assert!(!crate_exists(&mut conn, KRATE).await?);

        delete_version(&mut conn, env.async_storage(), env.config(), KRATE, &V1).await?;

        assert!(!crate_exists(&mut conn, KRATE).await?);

        Ok(())
    }
}
