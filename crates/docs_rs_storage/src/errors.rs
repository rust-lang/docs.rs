#[derive(Debug, Copy, Clone, thiserror::Error)]
#[error("the size limit for the buffer was reached")]
pub struct SizeLimitReached;
