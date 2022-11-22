use std::borrow::Cow;

use crate::{
    db::PoolError,
    web::{page::WebPage, releases::Search, AxumErrorPage, ErrorPage},
};
use axum::{
    http::StatusCode,
    response::{IntoResponse, Response as AxumResponse},
};
use iron::{status::Status, Handler, IronError, IronResult, Request, Response};

#[derive(Debug, Copy, Clone, thiserror::Error)]
pub enum Nope {
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
    #[error("Internal server error")]
    InternalServerError,
}

impl From<Nope> for IronError {
    fn from(err: Nope) -> IronError {
        use iron::status;

        let status = match err {
            Nope::ResourceNotFound
            | Nope::BuildNotFound
            | Nope::CrateNotFound
            | Nope::OwnerNotFound
            | Nope::VersionNotFound => status::NotFound,
            Nope::InternalServerError => status::InternalServerError,
        };

        IronError::new(err, status)
    }
}

impl Handler for Nope {
    fn handle(&self, req: &mut Request) -> IronResult<Response> {
        match self {
            Nope::ResourceNotFound => {
                // user tried to navigate to a resource (doc page/file) that doesn't exist
                // TODO: Display the attempted page
                ErrorPage {
                    title: "The requested resource does not exist",
                    message: Some("no such resource".into()),
                    status: Status::NotFound,
                }
                .into_response(req)
            }

            Nope::BuildNotFound => ErrorPage {
                title: "The requested build does not exist",
                message: Some("no such build".into()),
                status: Status::NotFound,
            }
            .into_response(req),

            Nope::CrateNotFound => {
                // user tried to navigate to a crate that doesn't exist
                // TODO: Display the attempted crate and a link to a search for said crate
                ErrorPage {
                    title: "The requested crate does not exist",
                    message: Some("no such crate".into()),
                    status: Status::NotFound,
                }
                .into_response(req)
            }

            Nope::OwnerNotFound => ErrorPage {
                title: "The requested owner does not exist",
                message: Some("no such owner".into()),
                status: Status::NotFound,
            }
            .into_response(req),

            Nope::VersionNotFound => {
                // user tried to navigate to a crate with a version that does not exist
                // TODO: Display the attempted crate and version
                ErrorPage {
                    title: "The requested version does not exist",
                    message: Some("no such version for this crate".into()),
                    status: Status::NotFound,
                }
                .into_response(req)
            }

            Nope::InternalServerError => {
                // something went wrong, details should have been logged
                ErrorPage {
                    title: "Internal server error",
                    message: Some("internal server error".into()),
                    status: Status::InternalServerError,
                }
                .into_response(req)
            }
        }
    }
}

impl From<PoolError> for IronError {
    fn from(err: PoolError) -> IronError {
        IronError::new(err, Status::InternalServerError)
    }
}

#[derive(Debug, thiserror::Error)]
#[allow(dead_code)] // FIXME: remove after iron is gone
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
    #[error("Search yielded no results")]
    NoResults,
    #[error("Internal server error")]
    InternalServerError,
    #[error("internal error")]
    InternalError(anyhow::Error),
}

impl IntoResponse for AxumNope {
    fn into_response(self) -> AxumResponse {
        match self {
            AxumNope::ResourceNotFound => {
                // user tried to navigate to a resource (doc page/file) that doesn't exist
                AxumErrorPage {
                    title: "Requested resource does not exist",
                    message: "no such resource".into(),
                    status: StatusCode::NOT_FOUND,
                }
                .into_response()
            }

            AxumNope::BuildNotFound => AxumErrorPage {
                title: "The requested build does not exist",
                message: "no such build".into(),
                status: StatusCode::NOT_FOUND,
            }
            .into_response(),

            AxumNope::CrateNotFound => {
                // user tried to navigate to a crate that doesn't exist
                // TODO: Display the attempted crate and a link to a search for said crate
                AxumErrorPage {
                    title: "The requested crate does not exist",
                    message: "no such crate".into(),
                    status: StatusCode::NOT_FOUND,
                }
                .into_response()
            }

            AxumNope::OwnerNotFound => AxumErrorPage {
                title: "The requested owner does not exist",
                message: "no such owner".into(),
                status: StatusCode::NOT_FOUND,
            }
            .into_response(),

            AxumNope::VersionNotFound => {
                // user tried to navigate to a crate with a version that does not exist
                // TODO: Display the attempted crate and version
                AxumErrorPage {
                    title: "The requested version does not exist",
                    message: "no such version for this crate".into(),
                    status: StatusCode::NOT_FOUND,
                }
                .into_response()
            }
            AxumNope::NoResults => {
                // user did a search with no search terms
                Search {
                    title: "No results given for empty search query".to_owned(),
                    status: StatusCode::NOT_FOUND,
                    ..Default::default()
                }
                .into_response()
            }
            AxumNope::InternalServerError => {
                // something went wrong, details should have been logged
                AxumErrorPage {
                    title: "Internal server error",
                    message: "internal server error".into(),
                    status: StatusCode::INTERNAL_SERVER_ERROR,
                }
                .into_response()
            }
            AxumNope::InternalError(source) => {
                let web_error = crate::web::AxumErrorPage {
                    title: "Internal Server Error",
                    message: Cow::Owned(source.to_string()),
                    status: StatusCode::INTERNAL_SERVER_ERROR,
                };

                crate::utils::report_error(&source);

                web_error.into_response()
            }
        }
    }
}

impl From<anyhow::Error> for AxumNope {
    fn from(err: anyhow::Error) -> Self {
        match err.downcast::<AxumNope>() {
            Ok(axum_nope) => axum_nope,
            Err(err) => match err.downcast::<Nope>() {
                Ok(iron_nope) => AxumNope::from(iron_nope),
                Err(err) => AxumNope::InternalError(err),
            },
        }
    }
}

impl From<Nope> for AxumNope {
    fn from(err: Nope) -> Self {
        match err {
            Nope::ResourceNotFound => AxumNope::ResourceNotFound,
            Nope::BuildNotFound => AxumNope::BuildNotFound,
            Nope::CrateNotFound => AxumNope::CrateNotFound,
            Nope::OwnerNotFound => AxumNope::OwnerNotFound,
            Nope::VersionNotFound => AxumNope::VersionNotFound,
            Nope::InternalServerError => AxumNope::InternalServerError,
        }
    }
}

pub(crate) type AxumResult<T> = Result<T, AxumNope>;

#[cfg(test)]
mod tests {
    use crate::test::wrapper;
    use kuchiki::traits::TendrilSink;

    #[test]
    fn check_404_page_content_crate() {
        wrapper(|env| {
            let page = kuchiki::parse_html().one(
                env.frontend()
                    .get("/crate-which-doesnt-exist")
                    .send()?
                    .text()?,
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
        wrapper(|env| {
            let page = kuchiki::parse_html().one(
                env.frontend()
                    .get("/resource-which-doesnt-exist.js")
                    .send()?
                    .text()?,
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
    fn check_404_page_content_not_semver_version() {
        wrapper(|env| {
            env.fake_release().name("dummy").create()?;
            let page =
                kuchiki::parse_html().one(env.frontend().get("/dummy/not-semver").send()?.text()?);
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
    fn check_404_page_content_nonexistent_version() {
        wrapper(|env| {
            env.fake_release().name("dummy").version("1.0.0").create()?;
            let page = kuchiki::parse_html().one(env.frontend().get("/dummy/2.0").send()?.text()?);
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
        wrapper(|env| {
            env.fake_release()
                .name("dummy")
                .version("1.0.0")
                .yanked(true)
                .create()?;
            let page = kuchiki::parse_html().one(env.frontend().get("/dummy/*").send()?.text()?);
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
