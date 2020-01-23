use super::Blob;
use failure::{Error, Fail};
use postgres::Connection;
use time::Timespec;

#[derive(Debug, Fail)]
#[fail(display = "the path is not present in the database")]
struct PathNotFoundError;

pub(crate) struct DatabaseBackend<'a> {
    conn: &'a Connection,
}

impl<'a> DatabaseBackend<'a> {
    pub(super) fn new(conn: &'a Connection) -> Self {
        Self { conn }
    }

    pub(super) fn get(&self, path: &str) -> Result<Blob, Error> {
        let rows = self.conn.query(
            "SELECT path, mime, date_updated, content FROM files WHERE path = $1;",
            &[&path],
        )?;

        if rows.is_empty() {
            Err(PathNotFoundError.into())
        } else {
            let row = rows.get(0);
            Ok(Blob {
                path: row.get("path"),
                mime: row.get("mime"),
                date_updated: row.get("date_updated"),
                content: row.get("content"),
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_path_get() {
        crate::test::wrapper(|env| {
            let conn = env.db().conn();
            let backend = DatabaseBackend::new(&conn);

            // Add a test file to the database
            conn.execute(
                "INSERT INTO files (path, mime, date_updated, content) VALUES ($1, $2, $3, $4);",
                &[
                    &"dir/foo.txt",
                    &"text/plain",
                    &Timespec::new(42, 0),
                    &"Hello world!".as_bytes(),
                ],
            )?;

            // Test that the proper file was returned
            assert_eq!(
                Blob {
                    path: "dir/foo.txt".into(),
                    mime: "text/plain".into(),
                    date_updated: Timespec::new(42, 0),
                    content: "Hello world!".bytes().collect(),
                },
                backend.get("dir/foo.txt")?
            );

            // Test that other files are not returned
            assert!(backend
                .get("dir/bar.txt")
                .unwrap_err()
                .downcast_ref::<PathNotFoundError>()
                .is_some());
            assert!(backend
                .get("foo.txt")
                .unwrap_err()
                .downcast_ref::<PathNotFoundError>()
                .is_some());

            Ok(())
        });
    }
}
