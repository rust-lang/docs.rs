//! a collection of custom extractors related to our app-context (context::Context)

use crate::error::AxumNope;
use axum::{
    extract::{FromRef, FromRequestParts},
    http::request::Parts,
};
use docs_rs_database::{AsyncPoolClient, Pool};
use std::ops::{Deref, DerefMut};

/// Extractor for a async sqlx database connection.
/// Can be used in normal axum handlers, middleware, or other extractors.
///
/// For now, we will retrieve a new connection each time the extractor is used.
///
/// This could be optimized in the future by caching the connection as a request
/// extension, so one request only uses on connection.
#[derive(Debug)]
pub(crate) struct DbConnection(AsyncPoolClient);

impl<S> FromRequestParts<S> for DbConnection
where
    S: Send + Sync,
    Pool: FromRef<S>,
{
    type Rejection = AxumNope;

    async fn from_request_parts(_parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        let pool = Pool::from_ref(state);

        Ok(Self(pool.get_async().await?))
    }
}

impl Deref for DbConnection {
    type Target = sqlx::PgConnection;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl DerefMut for DbConnection {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}
