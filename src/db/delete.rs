use crate::error::Result;
use crate::storage::{rustdoc_archive_path, source_archive_path, Storage};
use crate::Config;
use anyhow::Context as _;
use fn_error_context::context;
use postgres::Client;
use std::fs;

/// List of directories in docs.rs's underlying storage (either the database or S3) containing a
/// subdirectory named after the crate. Those subdirectories will be deleted.
static LIBRARY_STORAGE_PATHS_TO_DELETE: &[&str] = &["rustdoc", "sources"];
static BINARY_STORAGE_PATHS_TO_DELETE: &[&str] = &["sources"];

#[derive(Debug, thiserror::Error)]
enum CrateDeletionError {
    #[error("crate is missing: {0}")]
    MissingCrate(String),
}

#[context("error trying to delete crate {name} from database")]
pub fn delete_crate(
    conn: &mut Client,
    storage: &Storage,
    config: &Config,
    name: &str,
) -> Result<()> {
    let crate_id = get_id(conn, name)?;
    let is_library = delete_crate_from_database(conn, name, crate_id)?;
    // #899
    let paths = if is_library {
        LIBRARY_STORAGE_PATHS_TO_DELETE
    } else {
        BINARY_STORAGE_PATHS_TO_DELETE
    };

    for prefix in paths {
        // delete the whole rustdoc/source folder for this crate.
        // it will include existing archives.
        let remote_folder = format!("{prefix}/{name}/");
        storage.delete_prefix(&remote_folder)?;

        // remove existing local archive index files.
        let local_index_folder = config.local_archive_cache_path.join(&remote_folder);
        if local_index_folder.exists() {
            fs::remove_dir_all(&local_index_folder).with_context(|| {
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
pub fn delete_version(
    conn: &mut Client,
    storage: &Storage,
    config: &Config,
    name: &str,
    version: &str,
) -> Result<()> {
    let is_library = delete_version_from_database(conn, name, version)?;
    let paths = if is_library {
        LIBRARY_STORAGE_PATHS_TO_DELETE
    } else {
        BINARY_STORAGE_PATHS_TO_DELETE
    };

    for prefix in paths {
        storage.delete_prefix(&format!("{prefix}/{name}/{version}/"))?;
    }

    let local_archive_cache = &config.local_archive_cache_path;
    let mut paths = vec![source_archive_path(name, version)];
    if is_library {
        paths.push(rustdoc_archive_path(name, version));
    }

    for archive_filename in paths {
        // delete remove archive and remote index
        storage.delete_prefix(&archive_filename)?;

        // delete eventually existing local indexes
        let local_index_file = local_archive_cache.join(format!("{archive_filename}.index"));
        if local_index_file.exists() {
            fs::remove_file(&local_index_file).with_context(|| {
                format!("error when trying to remove local index: {local_index_file:?}")
            })?;
        }
    }

    Ok(())
}

fn get_id(conn: &mut Client, name: &str) -> Result<i32> {
    let crate_id_res = conn.query("SELECT id FROM crates WHERE name = $1", &[&name])?;
    if let Some(row) = crate_id_res.into_iter().next() {
        Ok(row.get("id"))
    } else {
        Err(CrateDeletionError::MissingCrate(name.into()).into())
    }
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
fn delete_version_from_database(conn: &mut Client, name: &str, version: &str) -> Result<bool> {
    let crate_id = get_id(conn, name)?;
    let mut transaction = conn.transaction()?;
    for &(table, column) in METADATA {
        transaction.execute(
            format!("DELETE FROM {table} WHERE {column} IN (SELECT id FROM releases WHERE crate_id = $1 AND version = $2)").as_str(),
            &[&crate_id, &version],
        )?;
    }
    let is_library: bool = transaction
        .query_one(
            "DELETE FROM releases WHERE crate_id = $1 AND version = $2 RETURNING is_library",
            &[&crate_id, &version],
        )?
        .get("is_library");
    transaction.execute(
        "UPDATE crates SET latest_version_id = (
            SELECT id FROM releases WHERE release_time = (
                SELECT MAX(release_time) FROM releases WHERE crate_id = $1
            )
        ) WHERE id = $1",
        &[&crate_id],
    )?;

    let paths = if is_library {
        LIBRARY_STORAGE_PATHS_TO_DELETE
    } else {
        BINARY_STORAGE_PATHS_TO_DELETE
    };
    for prefix in paths {
        transaction.execute(
            "DELETE FROM files WHERE path LIKE $1;",
            &[&format!("{prefix}/{name}/{version}/%")],
        )?;
    }

    transaction.commit()?;
    Ok(is_library)
}

/// Returns whether any release in this crate was a library
fn delete_crate_from_database(conn: &mut Client, name: &str, crate_id: i32) -> Result<bool> {
    let mut transaction = conn.transaction()?;

    transaction.execute(
        "DELETE FROM sandbox_overrides WHERE crate_name = $1",
        &[&name],
    )?;
    for &(table, column) in METADATA {
        transaction.execute(
            format!(
                "DELETE FROM {table} WHERE {column} IN (SELECT id FROM releases WHERE crate_id = $1)"
            )
            .as_str(),
            &[&crate_id],
        )?;
    }
    transaction.execute("DELETE FROM owner_rels WHERE cid = $1;", &[&crate_id])?;
    let has_library = transaction
        .query_one(
            "SELECT BOOL_OR(releases.is_library) AS has_library FROM releases",
            &[],
        )?
        .get("has_library");
    transaction.execute("DELETE FROM releases WHERE crate_id = $1;", &[&crate_id])?;
    transaction.execute("DELETE FROM crates WHERE id = $1;", &[&crate_id])?;

    // Transactions automatically rollback when not committing, so if any of the previous queries
    // fail the whole transaction will be aborted.
    transaction.commit()?;
    Ok(has_library)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::index::api::CrateOwner;
    use crate::test::{assert_success, wrapper};
    use postgres::Client;
    use test_case::test_case;

    fn crate_exists(conn: &mut Client, name: &str) -> Result<bool> {
        Ok(!conn
            .query("SELECT * FROM crates WHERE name = $1;", &[&name])?
            .is_empty())
    }

    fn release_exists(conn: &mut Client, id: i32) -> Result<bool> {
        Ok(!conn
            .query("SELECT * FROM releases WHERE id = $1;", &[&id])?
            .is_empty())
    }

    #[test_case(true)]
    #[test_case(false)]
    fn test_delete_crate(archive_storage: bool) {
        wrapper(|env| {
            let db = env.db();

            // Create fake packages in the database
            let pkg1_v1_id = env
                .fake_release()
                .name("package-1")
                .version("1.0.0")
                .archive_storage(archive_storage)
                .create()?;
            let pkg1_v2_id = env
                .fake_release()
                .name("package-1")
                .version("2.0.0")
                .archive_storage(archive_storage)
                .create()?;
            let pkg2_id = env
                .fake_release()
                .name("package-2")
                .archive_storage(archive_storage)
                .create()?;

            assert!(crate_exists(&mut db.conn(), "package-1")?);
            assert!(crate_exists(&mut db.conn(), "package-2")?);
            assert!(release_exists(&mut db.conn(), pkg1_v1_id)?);
            assert!(release_exists(&mut db.conn(), pkg1_v2_id)?);
            assert!(release_exists(&mut db.conn(), pkg2_id)?);
            for (pkg, version) in &[
                ("package-1", "1.0.0"),
                ("package-1", "2.0.0"),
                ("package-2", "1.0.0"),
            ] {
                assert!(env.storage().rustdoc_file_exists(
                    pkg,
                    version,
                    &format!("{pkg}/index.html"),
                    archive_storage
                )?);
            }

            delete_crate(&mut db.conn(), &env.storage(), &env.config(), "package-1")?;

            assert!(!crate_exists(&mut db.conn(), "package-1")?);
            assert!(crate_exists(&mut db.conn(), "package-2")?);
            assert!(!release_exists(&mut db.conn(), pkg1_v1_id)?);
            assert!(!release_exists(&mut db.conn(), pkg1_v2_id)?);
            assert!(release_exists(&mut db.conn(), pkg2_id)?);

            // files for package 2 still exists
            assert!(env.storage().rustdoc_file_exists(
                "package-2",
                "1.0.0",
                "package-2/index.html",
                archive_storage
            )?);

            // files for package 1 are gone
            if archive_storage {
                assert!(!env
                    .storage()
                    .exists(&rustdoc_archive_path("package-1", "1.0.0"))?);
                assert!(!env
                    .storage()
                    .exists(&rustdoc_archive_path("package-1", "2.0.0"))?);
            } else {
                assert!(!env.storage().rustdoc_file_exists(
                    "package-1",
                    "1.0.0",
                    "package-1/index.html",
                    archive_storage
                )?);
                assert!(!env.storage().rustdoc_file_exists(
                    "package-1",
                    "2.0.0",
                    "package-1/index.html",
                    archive_storage
                )?);
            }

            Ok(())
        });
    }

    #[test_case(true)]
    #[test_case(false)]
    fn test_delete_version(archive_storage: bool) {
        wrapper(|env| {
            fn owners(conn: &mut Client, crate_id: i32) -> Result<Vec<String>> {
                Ok(conn
                    .query(
                        "SELECT login FROM owners
                        INNER JOIN owner_rels ON owners.id = owner_rels.oid
                        WHERE owner_rels.cid = $1",
                        &[&crate_id],
                    )?
                    .into_iter()
                    .map(|row| row.get(0))
                    .collect())
            }

            let db = env.db();
            let v1 = env
                .fake_release()
                .name("a")
                .version("1.0.0")
                .archive_storage(archive_storage)
                .add_owner(CrateOwner {
                    login: "malicious actor".into(),
                    avatar: "https://example.org/malicious".into(),
                })
                .create()?;
            assert!(release_exists(&mut db.conn(), v1)?);
            assert!(env.storage().rustdoc_file_exists(
                "a",
                "1.0.0",
                "a/index.html",
                archive_storage
            )?);
            let crate_id = db
                .conn()
                .query("SELECT crate_id FROM releases WHERE id = $1", &[&v1])?
                .into_iter()
                .next()
                .unwrap()
                .get(0);
            assert_eq!(
                owners(&mut db.conn(), crate_id)?,
                vec!["malicious actor".to_string()]
            );

            let v2 = env
                .fake_release()
                .name("a")
                .version("2.0.0")
                .archive_storage(archive_storage)
                .add_owner(CrateOwner {
                    login: "Peter Rabbit".into(),
                    avatar: "https://example.org/peter".into(),
                })
                .create()?;
            assert!(release_exists(&mut db.conn(), v2)?);
            assert!(env.storage().rustdoc_file_exists(
                "a",
                "2.0.0",
                "a/index.html",
                archive_storage
            )?);
            assert_eq!(
                owners(&mut db.conn(), crate_id)?,
                vec!["Peter Rabbit".to_string()]
            );

            delete_version(&mut db.conn(), &env.storage(), &env.config(), "a", "1.0.0")?;
            assert!(!release_exists(&mut db.conn(), v1)?);
            if archive_storage {
                // for archive storage the archive and index files
                // need to be cleaned up.
                let rustdoc_archive = rustdoc_archive_path("a", "1.0.0");
                assert!(!env.storage().exists(&rustdoc_archive)?);

                // local and remote index are gone too
                let archive_index = format!("{rustdoc_archive}.index");
                assert!(!env.storage().exists(&archive_index)?);
                assert!(!env
                    .config()
                    .local_archive_cache_path
                    .join(&archive_index)
                    .exists());
            } else {
                assert!(!env.storage().rustdoc_file_exists(
                    "a",
                    "1.0.0",
                    "a/index.html",
                    archive_storage
                )?);
            }
            assert!(release_exists(&mut db.conn(), v2)?);
            assert!(env.storage().rustdoc_file_exists(
                "a",
                "2.0.0",
                "a/index.html",
                archive_storage
            )?);
            assert_eq!(
                owners(&mut db.conn(), crate_id)?,
                vec!["Peter Rabbit".to_string()]
            );

            let web = env.frontend();
            assert_success("/a/2.0.0/a/", web)?;
            assert_eq!(web.get("/a/1.0.0/a/").send()?.status(), 404);

            Ok(())
        })
    }
}
