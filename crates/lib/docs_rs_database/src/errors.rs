#[derive(Debug, thiserror::Error)]
pub enum PoolError {
    #[error("failed to create the database connection pool")]
    AsyncPoolCreationFailed(#[source] sqlx::Error),

    #[error("failed to get a database connection")]
    AsyncClientError(#[source] sqlx::Error),
}
