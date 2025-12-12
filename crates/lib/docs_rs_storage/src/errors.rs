#[derive(Debug, Copy, Clone, thiserror::Error)]
#[error("the size limit for the buffer was reached")]
pub struct SizeLimitReached;

#[derive(Debug, thiserror::Error)]
#[error("path not found")]
pub struct PathNotFoundError;
