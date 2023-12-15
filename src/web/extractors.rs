use crate::db::{AsyncPoolClient, Pool};
use anyhow::Context as _;
use axum::{
    async_trait,
    extract::{Extension, FromRequestParts},
    http::request::Parts,
    RequestPartsExt,
};
use std::ops::{Deref, DerefMut};

use super::error::AxumNope;

/// Extractor for a async sqlx database connection.
/// Can be used in normal axum handlers, middleware, or other extractors.
///
/// For now, we will retrieve a new connection each time the extractor is used.
///
/// This could be optimized in the future by caching the connection as a request
/// extension, so one request only uses on connection.
#[derive(Debug)]
pub(crate) struct DbConnection(AsyncPoolClient);

#[async_trait]
impl<S> FromRequestParts<S> for DbConnection
where
    S: Send + Sync,
{
    type Rejection = AxumNope;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        let Extension(pool) = parts
            .extract::<Extension<Pool>>()
            .await
            .context("could not extract pool extension")?;

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

/// custom axum `Path` extractor that uses our own AxumNope::BadRequest
/// as error response instead of a plain text "bad request"
#[allow(clippy::disallowed_types)]
mod path_impl {
    use super::*;

    #[derive(FromRequestParts)]
    #[from_request(via(axum::extract::Path), rejection(AxumNope))]
    pub(crate) struct Path<T>(pub T);
}

pub(crate) use path_impl::Path;

impl From<axum::extract::rejection::PathRejection> for AxumNope {
    fn from(value: axum::extract::rejection::PathRejection) -> Self {
        AxumNope::BadRequest(value.into())
    }
}

// TODO: we will write tests for this when async db tests are working
