//! Database based file handler

use super::pool::Pool;
use crate::{db, error::Result};
use iron::{status, Handler, IronError, IronResult, Request, Response};
use postgres::Connection;

const MAX_HTML_FILE_SIZE: usize = 5 * 1024 * 1024; // 5MB
const MAX_FILE_SIZE: usize = 50 * 1024 * 1024; // 50MB

pub(crate) struct File(pub(crate) db::file::Blob);

impl File {
    /// Gets file from database
    pub fn from_path(conn: &Connection, path: &str) -> Result<File> {
        let max_size = if path.ends_with(".html") {
            MAX_HTML_FILE_SIZE
        } else {
            MAX_FILE_SIZE
        };

        Ok(File(db::file::get_path(conn, path, max_size)?))
    }

    /// Consumes File and creates a iron response
    pub fn serve(self) -> Response {
        use iron::headers::{CacheControl, CacheDirective, ContentType, HttpDate, LastModified};

        let mut response = Response::with((status::Ok, self.0.content));
        let cache = vec![
            CacheDirective::Public,
            CacheDirective::MaxAge(super::STATIC_FILE_CACHE_DURATION as u32),
        ];
        response
            .headers
            .set(ContentType(self.0.mime.parse().unwrap()));
        response.headers.set(CacheControl(cache));
        // FIXME: This is so horrible
        response.headers.set(LastModified(HttpDate(
            time::strptime(
                &self.0.date_updated.format("%a, %d %b %Y %T %Z").to_string(),
                "%a, %d %b %Y %T %Z",
            )
            .unwrap(),
        )));
        response
    }

    /// Checks if mime type of file is "application/x-empty"
    pub fn is_empty(&self) -> bool {
        self.0.mime == "application/x-empty"
    }
}

/// Database based file handler for iron
///
/// This is similar to staticfile crate, but its using getting files from database.
pub struct DatabaseFileHandler;

impl Handler for DatabaseFileHandler {
    fn handle(&self, req: &mut Request) -> IronResult<Response> {
        let path = req.url.path().join("/");
        let conn = extension!(req, Pool).get()?;
        if let Ok(file) = File::from_path(&conn, &path) {
            Ok(file.serve())
        } else {
            Err(IronError::new(
                super::error::Nope::CrateNotFound,
                status::NotFound,
            ))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test::wrapper;
    use chrono::Utc;

    #[test]
    fn file_roundtrip() {
        wrapper(|env| {
            let db = env.db();
            let now = Utc::now();

            db.fake_release().create()?;

            let mut file = File::from_path(
                &*db.conn(),
                "rustdoc/fake-package/1.0.0/fake-package/index.html",
            )
            .unwrap();
            file.0.date_updated = now;

            let resp = file.serve();
            assert_eq!(
                resp.headers.get_raw("Last-Modified").unwrap(),
                [now.format("%a, %d %b %Y %T GMT").to_string().into_bytes()].as_ref(),
            );

            Ok(())
        });
    }
}
