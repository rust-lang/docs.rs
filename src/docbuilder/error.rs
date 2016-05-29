

use std::{io, fmt};
use std::error::Error;
use rustc_serialize::json::BuilderError;
use postgres;
use cargo;

#[derive(Debug)]
pub enum DocBuilderError {
    Io(io::Error),
    Json(BuilderError),
    JsonNotObject,
    JsonNameNotFound,
    JsonVersNotFound,
    FileNotFound,
    CargoError(Box<cargo::CargoError>),
    DatabaseConnectError(postgres::error::ConnectError),
    DatabaseError(postgres::error::Error),
}


impl fmt::Display for DocBuilderError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match *self {
            DocBuilderError::Io(ref err) => write!(f, "IO errors: {}", err),
            DocBuilderError::Json(ref err) => write!(f, "JSON builder errors: {}", err),
            DocBuilderError::JsonNotObject => write!(f, "JSON error NotObject"),
            DocBuilderError::JsonNameNotFound => write!(f, "JSON error NameNotFound"),
            DocBuilderError::JsonVersNotFound => write!(f, "JSON error VersNotFound"),
            DocBuilderError::FileNotFound => write!(f, "FileNotFound"),
            DocBuilderError::CargoError(ref err) => write!(f, "Cargo error: {}", err),
            DocBuilderError::DatabaseConnectError(ref err) => {
                write!(f, "Database connection error: {}", err)
            }
            DocBuilderError::DatabaseError(ref err) => write!(f, "Database error: {}", err),
        }
    }
}


impl Error for DocBuilderError {
    fn description(&self) -> &str {
        match *self {
            DocBuilderError::Io(ref err) => err.description(),
            DocBuilderError::Json(ref err) => err.description(),
            DocBuilderError::JsonNotObject => "JSON error NotObject",
            DocBuilderError::JsonNameNotFound => "JSON error NameNotFound",
            DocBuilderError::JsonVersNotFound => "JSON error VersNotFound",
            DocBuilderError::FileNotFound => "FileNotFound",
            DocBuilderError::CargoError(ref err) => err.description(),
            DocBuilderError::DatabaseConnectError(ref err) => err.description(),
            DocBuilderError::DatabaseError(ref err) => err.description(),
        }
    }

    fn cause(&self) -> Option<&Error> {
        match *self {
            DocBuilderError::Io(ref err) => Some(err),
            DocBuilderError::Json(ref err) => Some(err),
            DocBuilderError::JsonNotObject => None,
            DocBuilderError::JsonNameNotFound => None,
            DocBuilderError::JsonVersNotFound => None,
            DocBuilderError::FileNotFound => None,
            DocBuilderError::CargoError(ref err) => Some(err),
            DocBuilderError::DatabaseConnectError(ref err) => Some(err),
            DocBuilderError::DatabaseError(ref err) => Some(err),
        }
    }
}


impl From<io::Error> for DocBuilderError {
    fn from(err: io::Error) -> DocBuilderError {
        DocBuilderError::Io(err)
    }
}

impl From<BuilderError> for DocBuilderError {
    fn from(err: BuilderError) -> DocBuilderError {
        DocBuilderError::Json(err)
    }
}


impl From<Box<cargo::CargoError>> for DocBuilderError {
    fn from(err: Box<cargo::CargoError>) -> DocBuilderError {
        DocBuilderError::CargoError(err)
    }
}


impl From<postgres::error::ConnectError> for DocBuilderError {
    fn from(err: postgres::error::ConnectError) -> DocBuilderError {
        DocBuilderError::DatabaseConnectError(err)
    }
}

impl From<postgres::error::Error> for DocBuilderError {
    fn from(err: postgres::error::Error) -> DocBuilderError {
        DocBuilderError::DatabaseError(err)
    }
}
