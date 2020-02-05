//! Errors used in cratesfyi

use std::result::Result as StdResult;

pub(crate) use failure::Error;

pub type Result<T> = StdResult<T, Error>;
