#[macro_export]
macro_rules! block_on_async_with_conn {
    ($env:expr, |mut $conn:ident| async $body:block) => {{
        $env.runtime().block_on(async {
            let mut __conn = $env.db.async_conn().await?;
            let $conn: &mut sqlx::PgConnection = &mut *__conn;
            // Force the async block to yield anyhow::Result<_>
            let __res: anyhow::Result<_> = (async $body).await;
            __res
        })
    }};
}
