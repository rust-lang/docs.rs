//! Retrying JSON GET requests with a bounded, shared cache.

mod cached_result;
mod client;

#[cfg(any(test, feature = "testing"))]
pub mod testing;

pub use cached_result::CachedResult;
pub use client::Client;
