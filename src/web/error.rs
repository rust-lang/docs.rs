use crate::{
    db::PoolError,
    web::{page::WebPage, releases::Search, ErrorPage},
};
use failure::Fail;
use iron::{status::Status, Handler, IronError, IronResult, Request, Response};
use std::{error::Error, fmt};

#[derive(Debug, Copy, Clone)]
pub enum Nope {
    ResourceNotFound,
    CrateNotFound,
    VersionNotFound,
    NoResults,
    InternalServerError,
}

impl fmt::Display for Nope {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str(match *self {
            Nope::ResourceNotFound => "Requested resource not found",
            Nope::CrateNotFound => "Requested crate not found",
            Nope::VersionNotFound => "Requested crate does not have specified version",
            Nope::NoResults => "Search yielded no results",
            Nope::InternalServerError => "Internal server error",
        })
    }
}

impl Error for Nope {}

impl Handler for Nope {
    fn handle(&self, req: &mut Request) -> IronResult<Response> {
        match *self {
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

            Nope::NoResults => {
                let mut params = req.url.as_ref().query_pairs();

                if let Some((_, query)) = params.find(|(key, _)| key == "query") {
                    // this used to be a search
                    Search {
                        title: format!("No crates found matching '{}'", query),
                        search_query: Some(query.into_owned()),
                        status: Status::NotFound,
                        ..Default::default()
                    }
                    .into_response(req)
                } else {
                    // user did a search with no search terms
                    Search {
                        title: "No results given for empty search query".to_owned(),
                        status: Status::NotFound,
                        ..Default::default()
                    }
                    .into_response(req)
                }
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
        IronError::new(err.compat(), Status::InternalServerError)
    }
}

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
        // Resources with a `.js` and `.ico` extension are special cased in the
        // routes_handler which is currently run last. This means that `get("resource.exe")` will
        // fail with a `no so such crate` instead of 'no such resource'
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
