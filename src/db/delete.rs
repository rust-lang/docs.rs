use crate::storage::s3::{s3_client, S3_BUCKET_NAME};
use failure::{Error, Fail};
use postgres::{transaction::Transaction, Connection};
use rusoto_s3::{DeleteObjectsRequest, ListObjectsV2Request, ObjectIdentifier, S3Client, S3};

/// List of directories in docs.rs's underlying storage (either the database or S3) containing a
/// subdirectory named after the crate. Those subdirectories will be deleted.
static STORAGE_PATHS_TO_DELETE: &[&str] = &["rustdoc", "sources"];

#[derive(Debug, Fail)]
enum CrateDeletionError {
    #[fail(display = "crate is missing: {}", _0)]
    MissingCrate(String),
}

pub fn delete_crate(conn: &Connection, name: &str) -> Result<(), Error> {
    let crate_id = get_id(conn, name)?;
    delete_crate_from_database(conn, name, crate_id)?;
    if let Some(s3) = s3_client() {
        for prefix in STORAGE_PATHS_TO_DELETE {
            delete_prefix_from_s3(&s3, &format!("{}/{}/", prefix, name))?;
        }
    }
    Ok(())
}

pub fn delete_version(conn: &Connection, name: &str, version: &str) -> Result<(), Error> {
    delete_version_from_database(conn, name, version)?;

    if let Some(s3) = s3_client() {
        for prefix in STORAGE_PATHS_TO_DELETE {
            delete_prefix_from_s3(&s3, &format!("{}/{}/{}/", prefix, name, version))?;
        }
    }

    Ok(())
}

fn get_id(conn: &Connection, name: &str) -> Result<i32, Error> {
    let crate_id_res = conn.query("SELECT id FROM crates WHERE name = $1", &[&name])?;
    if let Some(row) = crate_id_res.into_iter().next() {
        Ok(row.get("id"))
    } else {
        Err(CrateDeletionError::MissingCrate(name.into()).into())
    }
}

// metaprogramming!
// NOTE: it is _crucial_ that table_name and column_name be hard-coded, i.e. NOT user input
fn delete_metadata(
    transaction: &Transaction,
    crate_id: i32,
    version: &str,
    table_name: &'static str,
    column_name: &'static str,
) -> Result<(), Error> {
    transaction.execute(
        &format!("DELETE FROM {} WHERE {} IN (SELECT id FROM releases WHERE crate_id = $1 AND version = $2)", table_name, column_name),
        &[&crate_id, &version],
    )?;
    Ok(())
}

fn delete_version_from_database(conn: &Connection, name: &str, version: &str) -> Result<(), Error> {
    let crate_id = get_id(conn, name)?;
    let transaction = conn.transaction()?;
    let metadata = [
        ("author_rels", "rid"),
        ("owner_rels", "cid"),
        ("keyword_rels", "rid"),
        ("builds", "rid"),
    ];
    for &(table, column) in &metadata {
        delete_metadata(&transaction, crate_id, version, table, column)?;
    }
    transaction.execute(
        "DELETE FROM releases WHERE crate_id = $1 AND version = $2",
        &[&crate_id, &version],
    )?;
    transaction.execute(
        "UPDATE crates SET latest_version_id = (
            SELECT id FROM releases WHERE release_time = (
                SELECT MAX(release_time) FROM releases WHERE crate_id = $1
            )
        ) WHERE id = $1",
        &[&crate_id],
    )?;

    for prefix in STORAGE_PATHS_TO_DELETE {
        transaction.execute(
            "DELETE FROM files WHERE path LIKE $1;",
            &[&format!("{}/{}/{}/%", prefix, name, version)],
        )?;
    }

    transaction.commit().map_err(Into::into)
}

fn delete_crate_from_database(conn: &Connection, name: &str, crate_id: i32) -> Result<(), Error> {
    let transaction = conn.transaction()?;

    transaction.execute(
        "DELETE FROM sandbox_overrides WHERE crate_name = $1",
        &[&name],
    )?;
    transaction.execute(
        "DELETE FROM author_rels WHERE rid IN (SELECT id FROM releases WHERE crate_id = $1);",
        &[&crate_id],
    )?;
    transaction.execute(
        "DELETE FROM owner_rels WHERE cid IN (SELECT id FROM releases WHERE crate_id = $1);",
        &[&crate_id],
    )?;
    transaction.execute(
        "DELETE FROM keyword_rels WHERE rid IN (SELECT id FROM releases WHERE crate_id = $1);",
        &[&crate_id],
    )?;
    transaction.execute(
        "DELETE FROM builds WHERE rid IN (SELECT id FROM releases WHERE crate_id = $1);",
        &[&crate_id],
    )?;
    transaction.execute("DELETE FROM releases WHERE crate_id = $1;", &[&crate_id])?;
    transaction.execute("DELETE FROM crates WHERE id = $1;", &[&crate_id])?;

    for prefix in STORAGE_PATHS_TO_DELETE {
        transaction.execute(
            "DELETE FROM files WHERE path LIKE $1;",
            &[&format!("{}/{}/%", prefix, name)],
        )?;
    }

    // Transactions automatically rollback when not committing, so if any of the previous queries
    // fail the whole transaction will be aborted.
    transaction.commit()?;
    Ok(())
}

fn delete_prefix_from_s3(s3: &S3Client, name: &str) -> Result<(), Error> {
    let mut continuation_token = None;
    loop {
        let list = s3
            .list_objects_v2(ListObjectsV2Request {
                bucket: S3_BUCKET_NAME.into(),
                prefix: Some(name.into()),
                continuation_token,
                ..ListObjectsV2Request::default()
            })
            .sync()?;

        let to_delete = list
            .contents
            .unwrap_or_else(Vec::new)
            .into_iter()
            .filter_map(|o| o.key)
            .map(|key| ObjectIdentifier {
                key,
                version_id: None,
            })
            .collect::<Vec<_>>();
        let resp = s3
            .delete_objects(DeleteObjectsRequest {
                bucket: S3_BUCKET_NAME.into(),
                delete: rusoto_s3::Delete {
                    objects: to_delete,
                    quiet: None,
                },
                ..DeleteObjectsRequest::default()
            })
            .sync()?;
        if let Some(errs) = resp.errors {
            for err in &errs {
                log::error!("error deleting file from s3: {:?}", err);
            }
            failure::bail!("uploading to s3 failed");
        }

        continuation_token = list.continuation_token;
        if continuation_token.is_none() {
            return Ok(());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test::{assert_success, wrapper};
    use failure::Error;
    use postgres::Connection;

    fn crate_exists(conn: &Connection, name: &str) -> Result<bool, Error> {
        Ok(!conn
            .query("SELECT * FROM crates WHERE name = $1;", &[&name])?
            .is_empty())
    }

    fn release_exists(conn: &Connection, id: i32) -> Result<bool, Error> {
        Ok(!conn
            .query("SELECT * FROM releases WHERE id = $1;", &[&id])?
            .is_empty())
    }

    #[test]
    fn test_delete_from_database() {
        wrapper(|env| {
            let db = env.db();

            // Create fake packages in the database
            let pkg1_v1_id = db
                .fake_release()
                .name("package-1")
                .version("1.0.0")
                .create()?;
            let pkg1_v2_id = db
                .fake_release()
                .name("package-1")
                .version("2.0.0")
                .create()?;
            let pkg2_id = db.fake_release().name("package-2").create()?;

            assert!(crate_exists(&db.conn(), "package-1")?);
            assert!(crate_exists(&db.conn(), "package-2")?);
            assert!(release_exists(&db.conn(), pkg1_v1_id)?);
            assert!(release_exists(&db.conn(), pkg1_v2_id)?);
            assert!(release_exists(&db.conn(), pkg2_id)?);

            let pkg1_id = &db
                .conn()
                .query("SELECT id FROM crates WHERE name = 'package-1';", &[])?
                .get(0)
                .get("id");

            delete_crate_from_database(&db.conn(), "package-1", *pkg1_id)?;

            assert!(!crate_exists(&db.conn(), "package-1")?);
            assert!(crate_exists(&db.conn(), "package-2")?);
            assert!(!release_exists(&db.conn(), pkg1_v1_id)?);
            assert!(!release_exists(&db.conn(), pkg1_v2_id)?);
            assert!(release_exists(&db.conn(), pkg2_id)?);

            Ok(())
        });
    }

    #[test]
    fn test_delete_version() {
        wrapper(|env| {
            fn authors(conn: &Connection, crate_id: i32) -> Result<Vec<String>, Error> {
                Ok(conn
                    .query(
                        "SELECT name FROM authors
                        INNER JOIN author_rels ON authors.id = author_rels.aid
                        INNER JOIN releases ON author_rels.rid = releases.id
                    WHERE releases.crate_id = $1",
                        &[&crate_id],
                    )?
                    .into_iter()
                    .map(|row| row.get(0))
                    .collect())
            }

            let db = env.db();
            let v1 = db
                .fake_release()
                .name("a")
                .version("1.0.0")
                .author("malicious actor")
                .create()?;
            let v2 = db
                .fake_release()
                .name("a")
                .version("2.0.0")
                .author("Peter Rabbit")
                .create()?;
            assert!(release_exists(&db.conn(), v1)?);
            assert!(release_exists(&db.conn(), v2)?);
            let crate_id = db
                .conn()
                .query("SELECT crate_id FROM releases WHERE id = $1", &[&v1])?
                .into_iter()
                .next()
                .unwrap()
                .get(0);
            assert_eq!(
                authors(&db.conn(), crate_id)?,
                vec!["malicious actor".to_string(), "Peter Rabbit".to_string()]
            );

            delete_version(&db.conn(), "a", "1.0.0")?;
            assert!(!release_exists(&db.conn(), v1)?);
            assert!(release_exists(&db.conn(), v2)?);
            assert_eq!(
                authors(&db.conn(), crate_id)?,
                vec!["Peter Rabbit".to_string()]
            );

            let web = env.frontend();
            assert_success("/a/2.0.0/a/", web)?;
            assert_eq!(web.get("/a/1.0.0/a/").send()?.status(), 404);

            Ok(())
        })
    }
}
