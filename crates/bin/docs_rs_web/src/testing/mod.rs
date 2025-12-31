pub(crate) mod axum_helpers;
pub(crate) mod headers;
mod test_env;

pub(crate) use axum_helpers::{AxumResponseTestExt, AxumRouterTestExt, assert_cache_headers_eq};
pub(crate) use test_env::TestEnvironment;

use std::rc::Rc;
use tokio::runtime;

/// legacy async wrapper, too much `.expect`.
/// Use `tokio::test`.
pub(crate) fn async_wrapper<F, Fut>(f: F)
where
    F: FnOnce(Rc<TestEnvironment>) -> Fut,
    Fut: Future<Output = anyhow::Result<()>>,
{
    let runtime = runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("failed to initialize runtime");

    let env = Rc::new(
        runtime
            .block_on(TestEnvironment::new())
            .expect("failed to initialize test environment"),
    );

    runtime.block_on(f(env.clone())).expect("test failed");
}
