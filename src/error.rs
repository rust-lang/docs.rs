//! Errors used in cratesfyi

use std::result::Result as StdResult;

pub use failure::{Error, ResultExt};

pub type Result<T> = StdResult<T, Error>;
