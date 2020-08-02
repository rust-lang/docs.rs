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
    NoResults,
    InternalServerError,
}

impl fmt::Display for Nope {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str(match *self {
            Nope::ResourceNotFound => "Requested resource not found",
            Nope::CrateNotFound => "Requested crate not found",
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
