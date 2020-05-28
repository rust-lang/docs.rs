use crate::web::page::{Error, Search, WebPage};
use iron::prelude::*;
use iron::status;
use iron::Handler;
use std::{error, fmt};

#[derive(Debug, Copy, Clone)]
pub enum Nope {
    ResourceNotFound,
    CrateNotFound,
    NoResults,
}

impl fmt::Display for Nope {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str(match *self {
            Nope::ResourceNotFound => "Requested resource not found",
            Nope::CrateNotFound => "Requested crate not found",
            Nope::NoResults => "Search yielded no results",
        })
    }
}

impl error::Error for Nope {}

impl Handler for Nope {
    fn handle(&self, req: &mut Request) -> IronResult<Response> {
        match *self {
            Nope::ResourceNotFound => {
                // user tried to navigate to a resource (doc page/file) that doesn't exist
                Error {
                    title: "The requested resource does not exist".to_owned(),
                    search_query: None,
                    status: status::NotFound,
                }
                .into_response()
            }

            Nope::CrateNotFound => {
                // user tried to navigate to a crate that doesn't exist
                Error {
                    title: "The requested crate does not exist".to_owned(),
                    search_query: None,
                    status: status::NotFound,
                }
                .into_response()
            }

            Nope::NoResults => {
                use params::{Params, Value};
                let params = req.get::<Params>().unwrap();
                if let Some(&Value::String(ref query)) = params.find(&["query"]) {
                    // this used to be a search
                    Search {
                        title: format!("No crates found matching '{}'", query),
                        search_query: Some(query.to_owned()),
                        status: status::NotFound,
                        ..Default::default()
                    }
                    .into_response()
                } else {
                    // user did a search with no search terms
                    Search {
                        title: "No results given for empty search query".to_owned(),
                        status: status::NotFound,
                        ..Default::default()
                    }
                    .into_response()
                }
            }
        }
    }
}
