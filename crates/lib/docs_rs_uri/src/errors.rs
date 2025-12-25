use std::str;

pub(crate) type Result<T> = std::result::Result<T, UriError>;

#[derive(Debug, thiserror::Error)]
pub enum UriError {
    #[error(transparent)]
    Utf8Error(#[from] str::Utf8Error),

    #[error(transparent)]
    InvalidUriError(#[from] http::uri::InvalidUri),

    #[error("can't convert EscapedURI with fragment into Uri")]
    Fragment,
}
