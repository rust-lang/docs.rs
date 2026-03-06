use tokio::runtime;
use tracing::Instrument as _;

/// Newtype around `tokio::runtime::Handle` that adds
/// missing integration with tracing spans.
#[derive(Debug, Clone)]
pub struct Handle(runtime::Handle);

impl Handle {
    pub fn block_on<F: Future>(&self, future: F) -> F::Output {
        runtime::Handle::block_on(self.as_handle(), future.in_current_span())
    }

    pub fn as_handle(&self) -> &runtime::Handle {
        &self.0
    }
}

impl From<runtime::Handle> for Handle {
    fn from(handle: runtime::Handle) -> Self {
        Handle(handle)
    }
}
