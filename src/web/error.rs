use std::error::Error;
use iron::prelude::*;
use iron::Handler;
use iron::status;
use web::page::Page;
use std::fmt;

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

impl Error for Nope {}

impl Handler for Nope {
    fn handle(&self, req: &mut Request) -> IronResult<Response> {
        match *self {
            Nope::ResourceNotFound => {
                // user tried to navigate to a resource (doc page/file) that doesn't exist
                Page::new("no such resource".to_owned())
                    .set_status(status::NotFound)
                    .title("The requested resource does not exist")
                    .to_resp("error")
            }
            Nope::CrateNotFound => {
                // user tried to navigate to a crate that doesn't exist
                Page::new("no such crate".to_owned())
                    .set_status(status::NotFound)
                    .title("The requested crate does not exist")
                    .to_resp("error")
            }
            Nope::NoResults => {
                use params::{Params, Value};
                let params = req.get::<Params>().unwrap();
                if let Some(&Value::String(ref query)) = params.find(&["query"]) {
                    // this used to be a search
                    Page::new(Vec::<super::releases::Release>::new())
                        .set_status(status::NotFound)
                        .set("search_query", &query)
                        .title(&format!("No crates found matching '{}'", query))
                        .to_resp("releases")
                } else {
                    // user did a search with no search terms
                    Page::new(Vec::<super::releases::Release>::new())
                        .set_status(status::NotFound)
                        .title("No results given for empty search query")
                        .to_resp("releases")
                }
            }
        }
    }
}
