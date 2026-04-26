use crate::{
    cache::CachePolicy,
    handlers::releases::Search,
    impl_axum_webpage,
    page::templates::{RenderBrands, RenderSolid},
};
use anyhow::{Result, anyhow};
use askama::Template;
use axum::{
    Json,
    http::StatusCode,
    response::{IntoResponse, Response as AxumResponse},
};
use docs_rs_database::PoolError;
use docs_rs_storage::PathNotFoundError;
use docs_rs_uri::EscapedURI;
use std::borrow::Cow;
use tracing::error;

#[derive(Template)]
#[template(path = "error.html")]
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct AxumErrorPage {
    /// The title of the page
    pub title: &'static str,
    /// The error message, displayed as a description
    pub message: Cow<'static, str>,
    pub status: StatusCode,
}

impl_axum_webpage! {
    AxumErrorPage,
    status = |err| err.status,

}

#[derive(Debug, thiserror::Error)]
pub enum AxumNope {
    #[error("Requested resource not found")]
    ResourceNotFound,
    #[error("Requested build not found")]
    BuildNotFound,
    #[error("Requested crate not found")]
    CrateNotFound,
    #[error("Requested owner not found")]
    OwnerNotFound,
    #[error("Requested crate does not have specified version")]
    VersionNotFound,
    #[error("Requested release doesn't have docs for the given target")]
    TargetNotFound,
    #[error("Search yielded no results")]
    NoResults,
    #[error("Unauthorized: {0}")]
    Unauthorized(&'static str),
    #[error("internal error")]
    InternalError(anyhow::Error),
    #[error("bad request")]
    BadRequest(anyhow::Error),
    #[error("redirect")]
    Redirect(EscapedURI, CachePolicy),
}

// FUTURE: Ideally, the split between the 3 kinds of responses would
// be done by having multiple nested enums in the first place instead
// of just `AxumNope`, to keep everything statically type-checked
// throughout instead of having the potential for a runtime error.

impl AxumNope {
    fn into_error_info(self) -> ErrorInfo {
        match self {
            AxumNope::ResourceNotFound => {
                // user tried to navigate to a resource (doc page/file) that doesn't exist
                ErrorInfo {
                    title: "The requested resource does not exist",
                    message: "no such resource".into(),
                    status: StatusCode::NOT_FOUND,
                }
            }
            AxumNope::BuildNotFound => ErrorInfo {
                title: "The requested build does not exist",
                message: "no such build".into(),
                status: StatusCode::NOT_FOUND,
            },
            AxumNope::TargetNotFound => {
                // user tried to navigate to a target that doesn't exist
                ErrorInfo {
                    title: "The requested target does not exist",
                    message: "no such target".into(),
                    status: StatusCode::NOT_FOUND,
                }
            }
            AxumNope::CrateNotFound => {
                // user tried to navigate to a crate that doesn't exist
                // TODO: Display the attempted crate and a link to a search for said crate
                ErrorInfo {
                    title: "The requested crate does not exist",
                    message: "no such crate".into(),
                    status: StatusCode::NOT_FOUND,
                }
            }
            AxumNope::OwnerNotFound => ErrorInfo {
                title: "The requested owner does not exist",
                message: "no such owner".into(),
                status: StatusCode::NOT_FOUND,
            },
            AxumNope::VersionNotFound => {
                // user tried to navigate to a crate with a version that does not exist
                // TODO: Display the attempted crate and version
                ErrorInfo {
                    title: "The requested version does not exist",
                    message: "no such version for this crate".into(),
                    status: StatusCode::NOT_FOUND,
                }
            }
            AxumNope::NoResults => {
                // user did a search with no search terms
                unreachable!()
            }
            AxumNope::BadRequest(source) => ErrorInfo {
                title: "Bad request",
                message: Cow::Owned(source.to_string()),
                status: StatusCode::BAD_REQUEST,
            },
            AxumNope::Unauthorized(what) => ErrorInfo {
                title: "Unauthorized",
                message: what.into(),
                status: StatusCode::UNAUTHORIZED,
            },
            AxumNope::InternalError(source) => {
                error!(?source, "internal server error");
                ErrorInfo {
                    title: "Internal Server Error",
                    message: Cow::Owned(source.to_string()),
                    status: StatusCode::INTERNAL_SERVER_ERROR,
                }
            }
            AxumNope::Redirect(_target, _cache_policy) => unreachable!(),
        }
    }
}

struct ErrorInfo {
    // For the title of the page
    pub title: &'static str,
    // The error message, displayed as a description
    pub message: Cow<'static, str>,
    // The status code of the response
    pub status: StatusCode,
}

fn redirect_with_policy(target: EscapedURI, cache_policy: CachePolicy) -> AxumResponse {
    match crate::handlers::axum_cached_redirect(target, cache_policy) {
        Ok(response) => response.into_response(),
        Err(err) => AxumNope::InternalError(err).into_response(),
    }
}

impl IntoResponse for AxumNope {
    fn into_response(self) -> AxumResponse {
        match self {
            AxumNope::NoResults => {
                // user did a search with no search terms
                Search {
                    title: "No results given for empty search query".to_owned(),
                    status: StatusCode::NOT_FOUND,
                    ..Default::default()
                }
                .into_response()
            }
            AxumNope::Redirect(target, cache_policy) => redirect_with_policy(target, cache_policy),
            _ => {
                let ErrorInfo {
                    title,
                    message,
                    status,
                } = self.into_error_info();
                AxumErrorPage {
                    title,
                    message,
                    status,
                }
                .into_response()
            }
        }
    }
}

/// `AxumNope` but generating error responses in JSON (for API).
pub(crate) struct JsonAxumNope(pub AxumNope);

impl IntoResponse for JsonAxumNope {
    fn into_response(self) -> AxumResponse {
        match self.0 {
            AxumNope::NoResults => {
                // user did a search with no search terms; invalid,
                // return 404
                StatusCode::NOT_FOUND.into_response()
            }
            AxumNope::Redirect(target, cache_policy) => redirect_with_policy(target, cache_policy),
            _ => {
                let ErrorInfo {
                    title,
                    message,
                    status,
                } = self.0.into_error_info();
                (
                    status,
                    Json(serde_json::json!({
                        "title": title,
                        "message": message,
                    })),
                )
                    .into_response()
            }
        }
    }
}

impl From<anyhow::Error> for AxumNope {
    fn from(err: anyhow::Error) -> Self {
        match err.downcast::<AxumNope>() {
            Ok(axum_nope) => axum_nope,
            Err(err) => match err.downcast::<PathNotFoundError>() {
                Ok(_) => AxumNope::ResourceNotFound,
                Err(err) => AxumNope::InternalError(err),
            },
        }
    }
}

impl From<sqlx::Error> for AxumNope {
    fn from(err: sqlx::Error) -> Self {
        AxumNope::InternalError(anyhow!(err))
    }
}

impl From<PoolError> for AxumNope {
    fn from(err: PoolError) -> Self {
        AxumNope::InternalError(anyhow!(err))
    }
}

pub(crate) type AxumResult<T> = Result<T, AxumNope>;
pub(crate) type JsonAxumResult<T> = Result<T, JsonAxumNope>;

#[cfg(test)]
mod tests {
    use super::{AxumNope, EscapedURI, IntoResponse};
    use crate::cache::CachePolicy;
    use crate::testing::{
        AxumResponseTestExt, AxumRouterTestExt, TestEnvironmentExt as _, async_wrapper,
    };
    use kuchikiki::traits::TendrilSink;

    #[test]
    fn test_redirect_error_encodes_url_path() {
        let response = AxumNope::Redirect(
            EscapedURI::from_path("/something>"),
            CachePolicy::ForeverInCdnAndBrowser,
        )
        .into_response();

        assert_eq!(response.status(), 302);
        assert_eq!(response.headers().get("Location").unwrap(), "/something%3E");
    }

    #[test]
    fn check_404_page_content_crate() {
        async_wrapper(|env| async move {
            let page = kuchikiki::parse_html().one(
                env.web_app()
                    .await
                    .get("/crate-which-doesnt-exist")
                    .await?
                    .text()
                    .await?,
            );
            assert_eq!(page.select("#crate-title").unwrap().count(), 1);
            assert_eq!(
                page.select("#crate-title")
                    .unwrap()
                    .next()
                    .unwrap()
                    .text_contents(),
                "The requested crate does not exist",
            );

            Ok(())
        });
    }

    #[test]
    fn check_404_page_content_resource() {
        async_wrapper(|env| async move {
            let page = kuchikiki::parse_html().one(
                env.web_app()
                    .await
                    .get("/resource-which-doesnt-exist.js")
                    .await?
                    .text()
                    .await?,
            );
            assert_eq!(page.select("#crate-title").unwrap().count(), 1);
            assert_eq!(
                page.select("#crate-title")
                    .unwrap()
                    .next()
                    .unwrap()
                    .text_contents(),
                "The requested resource does not exist",
            );

            Ok(())
        });
    }

    #[test]
    fn check_400_page_content_not_semver_version() {
        async_wrapper(|env| async move {
            env.fake_release().await.name("dummy").create().await?;

            let response = env.web_app().await.get("/dummy/not-semver").await?;
            assert_eq!(response.status(), 400);

            let page = kuchikiki::parse_html().one(response.text().await?);
            assert_eq!(page.select("#crate-title").unwrap().count(), 1);
            assert_eq!(
                page.select("#crate-title")
                    .unwrap()
                    .next()
                    .unwrap()
                    .text_contents(),
                "Bad request"
            );

            Ok(())
        });
    }

    #[test]
    fn check_404_page_content_nonexistent_version() {
        async_wrapper(|env| async move {
            env.fake_release()
                .await
                .name("dummy")
                .version("1.0.0")
                .create()
                .await?;
            let page = kuchikiki::parse_html()
                .one(env.web_app().await.get("/dummy/2.0").await?.text().await?);
            assert_eq!(page.select("#crate-title").unwrap().count(), 1);
            assert_eq!(
                page.select("#crate-title")
                    .unwrap()
                    .next()
                    .unwrap()
                    .text_contents(),
                "The requested version does not exist",
            );

            Ok(())
        });
    }

    #[test]
    fn check_404_page_content_any_version_all_yanked() {
        async_wrapper(|env| async move {
            env.fake_release()
                .await
                .name("dummy")
                .version("1.0.0")
                .yanked(true)
                .create()
                .await?;
            let page = kuchikiki::parse_html()
                .one(env.web_app().await.get("/dummy/*").await?.text().await?);
            assert_eq!(page.select("#crate-title").unwrap().count(), 1);
            assert_eq!(
                page.select("#crate-title")
                    .unwrap()
                    .next()
                    .unwrap()
                    .text_contents(),
                "The requested version does not exist",
            );

            Ok(())
        });
    }
}
