//! Errors used in cratesfyi

use std::result::Result as StdResult;

pub(crate) use failure::Error;

pub type Result<T> = StdResult<T, Error>;

#[derive(Debug, Copy, Clone)]
pub(crate) struct SizeLimitReached;

impl std::error::Error for SizeLimitReached {}

impl std::fmt::Display for SizeLimitReached {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "the size limit for the buffer was reached")
    }
}
