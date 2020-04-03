//! Database based file handler

use super::pool::Pool;
use time;
use postgres::Connection;
use iron::{Handler, Request, IronResult, Response, IronError};
use iron::status;
use crate::db;

pub struct File(pub db::file::Blob);

impl File {
    /// Gets file from database
    pub fn from_path(conn: &Connection, path: &str) -> Option<File> {
        Some(File(db::file::get_path(conn, path)?))
    }

    /// Consumes File and creates a iron response
    pub fn serve(self) -> Response {
        use iron::headers::{CacheControl, LastModified, CacheDirective, HttpDate, ContentType};

        let mut response = Response::with((status::Ok, self.0.content));
        let cache = vec![
            CacheDirective::Public,
            CacheDirective::MaxAge(super::STATIC_FILE_CACHE_DURATION as u32),
        ];
        response
            .headers
            .set(ContentType(self.0.mime.parse().unwrap()));
        response.headers.set(CacheControl(cache));
        response
            .headers
            .set(LastModified(HttpDate(time::at(self.0.date_updated))));
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
        let conn = extension!(req, Pool).get();
        if let Some(file) = File::from_path(&conn, &path) {
            Ok(file.serve())
        } else {
            Err(IronError::new(
                super::error::Nope::CrateNotFound,
                status::NotFound,
            ))
        }
    }
}
