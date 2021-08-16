//! Errors used in docs.rs

pub(crate) use anyhow::Result;

#[derive(Debug, Copy, Clone, thiserror::Error)]
#[error("the size limit for the buffer was reached")]
pub(crate) struct SizeLimitReached;
