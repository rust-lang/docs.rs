mod database;
mod s3;

pub(crate) use self::database::DatabaseBackend;
pub(crate) use self::s3::S3Backend;
use failure::Error;
use time::Timespec;

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Blob {
    pub(crate) path: String,
    pub(crate) mime: String,
    pub(crate) date_updated: Timespec,
    pub(crate) content: Vec<u8>,
}

pub(crate) enum Storage<'a> {
    Database(DatabaseBackend<'a>),
    S3(S3Backend<'a>),
}

impl Storage<'_> {
    pub(crate) fn get(&self, path: &str) -> Result<Blob, Error> {
        match self {
            Self::Database(db) => db.get(path),
            Self::S3(s3) => s3.get(path),
        }
    }
}

impl<'a> From<DatabaseBackend<'a>> for Storage<'a> {
    fn from(db: DatabaseBackend<'a>) -> Self {
        Self::Database(db)
    }
}

impl<'a> From<S3Backend<'a>> for Storage<'a> {
    fn from(db: S3Backend<'a>) -> Self {
        Self::S3(db)
    }
}
