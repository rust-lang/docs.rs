use std::ops::Deref;
use tokio::runtime;
use tracing::Instrument as _;

/// Newtype around `tokio::runtime::Handle` that adds
/// missing integration with tracing spans.
pub struct Handle(runtime::Handle);

impl Handle {
    pub fn block_on<F: Future>(&self, future: F) -> F::Output {
        runtime::Handle::block_on(self, future.in_current_span())
    }
}

impl From<runtime::Handle> for Handle {
    fn from(handle: runtime::Handle) -> Self {
        Handle(handle)
    }
}

impl Deref for Handle {
    type Target = runtime::Handle;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}
