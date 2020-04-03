use super::file::{s3_client, S3_BUCKET_NAME};
use failure::{Error, Fail};
use postgres::Connection;
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
    let crate_id_res = conn.query("SELECT id FROM crates WHERE name = $1", &[&name])?;
    let crate_id = if crate_id_res.is_empty() {
        return Err(CrateDeletionError::MissingCrate(name.into()).into());
    } else {
        crate_id_res.get(0).get("id")
    };

    delete_from_database(conn, name, crate_id)?;
    if let Some(s3) = s3_client() {
        delete_from_s3(&s3, name)?;
    }

    Ok(())
}

fn delete_from_database(conn: &Connection, name: &str, crate_id: i32) -> Result<(), Error> {
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

fn delete_from_s3(s3: &S3Client, name: &str) -> Result<(), Error> {
    for prefix in STORAGE_PATHS_TO_DELETE {
        delete_prefix_from_s3(s3, &format!("{}/{}/", prefix, name))?;
    }
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
    use failure::Error;
    use postgres::Connection;

    #[test]
    fn test_delete_from_database() {
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

        crate::test::wrapper(|env| {
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

            delete_from_database(&db.conn(), "package-1", *pkg1_id)?;

            assert!(!crate_exists(&db.conn(), "package-1")?);
            assert!(crate_exists(&db.conn(), "package-2")?);
            assert!(!release_exists(&db.conn(), pkg1_v1_id)?);
            assert!(!release_exists(&db.conn(), pkg1_v2_id)?);
            assert!(release_exists(&db.conn(), pkg2_id)?);

            Ok(())
        });
    }
}
