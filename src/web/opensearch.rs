use iron::headers::ContentType;
use iron::prelude::*;
use iron::status;

const OPENSEARCH_XML: &'static [u8] = include_bytes!("opensearch.xml");

pub fn serve_opensearch(_: &mut Request) -> IronResult<Response> {
    let mut response = Response::with((status::Ok, OPENSEARCH_XML));
    response.headers.set(ContentType("application/opensearchdescription+xml".parse().unwrap()));
    Ok(response)
}
