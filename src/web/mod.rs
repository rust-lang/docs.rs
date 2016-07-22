//! Web interface of cratesfyi


mod rustdoc;
mod releases;
mod page;
mod crate_details;
mod source;
mod pool;
mod file;
mod builds;
mod error;

use std::env;
use std::error::Error;
use std::time::Duration;
use std::path::PathBuf;
use iron::prelude::*;
use iron::Handler;
use router::Router;
use staticfile::Static;
use handlebars_iron::{HandlebarsEngine, DirectorySource};
use time;
use postgres::Connection;
use semver::{Version, VersionReq};
use rustc_serialize::json::{Json, ToJson};
use std::collections::BTreeMap;



/// Duration of static files for staticfile and DatabaseFileHandler (in seconds)
const STATIC_FILE_CACHE_DURATION: u64 = 60 * 60 * 24 * 3;   // 3 days


struct CratesfyiHandler {
    router_handler: Box<Handler>,
    database_file_handler: Box<Handler>,
    static_handler: Box<Handler>,
}


impl CratesfyiHandler {
    pub fn new() -> CratesfyiHandler {
        let mut router = Router::new();
        router.get("/", releases::home_page);
        router.get("/releases", releases::releases_handler);
        router.get("/releases/recent/:page", releases::releases_handler);
        router.get("/releases/stars", releases::stars_handler);
        router.get("/releases/stars/:page", releases::stars_handler);
        router.get("/releases/:author", releases::author_handler);
        router.get("/releases/:author/:page", releases::author_handler);
        router.get("/rustdoc/:crate", rustdoc::rustdoc_redirector_handler);
        router.get("/rustdoc/:crate/", rustdoc::rustdoc_redirector_handler);
        router.get("/rustdoc/:crate/:version", rustdoc::rustdoc_redirector_handler);
        router.get("/rustdoc/:crate/:version/", rustdoc::rustdoc_redirector_handler);
        router.get("/rustdoc/:crate/:version/*", rustdoc::rustdoc_html_server_handler);
        router.get("/crates/:name", crate_details::crate_details_handler);
        router.get("/crates/:name/", crate_details::crate_details_handler);
        router.get("/crates/:name/:version", crate_details::crate_details_handler);
        router.get("/crates/:name/:version/", crate_details::crate_details_handler);
        router.get("/crates/:name/:version/builds", builds::build_list_handler);
        router.get("/crates/:name/:version/builds/:id", builds::build_list_handler);
        router.get("/source/:name/:version/", source::source_browser_handler);
        router.get("/source/:name/:version/*", source::source_browser_handler);
        router.get("/search", releases::search_handler);

        // TODO: Use DocBuilderOptions for paths
        let mut hbse = HandlebarsEngine::new();
        hbse.add(Box::new(DirectorySource::new("./templates", ".hbs")));

        // load templates
        if let Err(e) = hbse.reload() {
            panic!("Failed to load handlebar templates: {}", e.description());
        }

        let mut router_chain = Chain::new(router);
        router_chain.link_before(pool::Pool::new());
        router_chain.link_after(hbse);

        let prefix = PathBuf::from(env::var("CRATESFYI_PREFIX").unwrap()).join("public_html");
        let static_handler = Static::new(prefix)
            .cache(Duration::from_secs(STATIC_FILE_CACHE_DURATION));

        CratesfyiHandler {
            router_handler: Box::new(router_chain),
            database_file_handler: Box::new(file::DatabaseFileHandler),
            static_handler: Box::new(static_handler),
        }
    }
}


impl Handler for CratesfyiHandler {
    fn handle(&self, req: &mut Request) -> IronResult<Response> {
        // try router first then db/static file handler
        // return 404 if none of them return Ok
        self.router_handler
            .handle(req)
            .or_else(|e| {
                // if router fails try to serve files from database first
                self.database_file_handler.handle(req).or(Err(e))
            })
            .or_else(|e| {
                // and then try static handler. if all of them fails, return 404
                self.static_handler.handle(req).or(Err(e))
            })
            .or_else(|e| {
                debug!("{}", e.description());
                let err: &error::Nope = e.error
                    .downcast::<error::Nope>()
                    .expect("all cratesfyi errors should be of type Nope");
                err.handle(req)
            })
    }
}



fn match_version(conn: &Connection, name: &str, version: Option<&str>) -> Option<String> {

    // version is an Option<&str> from router::Router::get
    // need to decode first
    use url::percent_encoding::percent_decode;
    let req_version = version.and_then(|v| {
        match percent_decode(v.as_bytes()).decode_utf8() {
            Ok(p) => Some(p.into_owned()),
            Err(_) => None,
        }
    }).unwrap_or("*".to_string());

    let versions = {

        let mut versions = Vec::new();
        // get every version of a crate
        for row in &conn.query("SELECT version  \
                                FROM releases \
                                INNER JOIN crates ON crates.id = releases.crate_id \
                                WHERE crates.name = $1",
                                &[&name])
            .unwrap() {
                let version: String = row.get(0);
                versions.push(version);
            }

        // FIXME: Need to sort versions with semver, database is not keeping them sorted
        versions
    };

    // first check for exact match
    // we can't expect users to use semver in query
    for version in &versions {
        if version == &req_version {
            return Some(version.clone())
        }
    }

    // Now try to match with semver
    let req_sem_ver = VersionReq::parse(&req_version).unwrap();

    // we need to sort versions first
    let versions_sem = {
        let mut versions_sem: Vec<Version> = Vec::new();

        for version in &versions {
            versions_sem.push(Version::parse(&version).unwrap());
        }

        versions_sem.sort();
        versions_sem.reverse();
        versions_sem
    };

    for version in &versions_sem {
        if req_sem_ver.matches(&version) {
            return Some(format!("{}", version))
        }
    }

    None
}





/// Wrapper around the pulldown-cmark parser and renderer to render markdown
fn render_markdown(text: &str) -> String {
    // I got this from mdBook::src::utils
    use pulldown_cmark::{Parser, html, Options, OPTION_ENABLE_TABLES, OPTION_ENABLE_FOOTNOTES};
    let mut s = String::with_capacity(text.len() * 3 / 2);

    let mut opts = Options::empty();
    opts.insert(OPTION_ENABLE_TABLES);
    opts.insert(OPTION_ENABLE_FOOTNOTES);

    let p = Parser::new_ext(&text, opts);
    html::push_html(&mut s, p);
    s
}



/// Starts cratesfyi web server
pub fn start_web_server() {
    let cratesfyi = CratesfyiHandler::new();
    Iron::new(cratesfyi).http("localhost:3000").unwrap();
}



/// Converts Timespec to nice readable relative time string
fn duration_to_str(ts: time::Timespec) -> String {

    let tm = time::at(ts);
    let delta = time::now() - tm;

    if delta.num_days() > 5 {
        format!("{}", tm.strftime("%b %d, %Y").unwrap())
    } else if delta.num_days() > 1 {
        format!("{} days ago", delta.num_days())
    } else if delta.num_days() == 1 {
        "one day ago".to_string()
    } else if delta.num_hours() > 1 {
        format!("{} hours ago", delta.num_hours())
    } else if delta.num_hours() == 1 {
        "an hour ago".to_string()
    } else if delta.num_minutes() > 1 {
        format!("{} minutes ago", delta.num_minutes())
    } else if delta.num_minutes() == 1 {
        "one minute ago".to_string()
    } else if delta.num_seconds() > 0 {
        format!("{} seconds ago", delta.num_seconds())
    } else {
        "just now".to_string()
    }

}



/// MetaData used in header
#[derive(Debug)]
pub struct MetaData {
    pub name: String,
    pub version: String,
    pub description: Option<String>,
    pub target_name: Option<String>,
    pub rustdoc_status: bool,
}


impl MetaData {
    pub fn from_crate(conn: &Connection, name: &str, version: &str) -> Option<MetaData> {
        for row in &conn.query("SELECT crates.name,
                                       releases.version,
                                       releases.description,
                                       releases.target_name,
                                       releases.rustdoc_status
                                FROM releases
                                INNER JOIN crates ON crates.id = releases.crate_id
                                WHERE crates.name = $1 AND releases.version = $2",
                               &[&name, &version]).unwrap() {

            return Some(MetaData {
                name: row.get(0),
                version: row.get(1),
                description: row.get(2),
                target_name: row.get(3),
                rustdoc_status: row.get(4),
            });
        }

        None
    }
}


impl ToJson for MetaData {
    fn to_json(&self) -> Json {
        let mut m: BTreeMap<String, Json> = BTreeMap::new();
        m.insert("name".to_owned(), self.name.to_json());
        m.insert("version".to_owned(), self.version.to_json());
        m.insert("description".to_owned(), self.description.to_json());
        m.insert("target_name".to_owned(), self.target_name.to_json());
        m.insert("rustdoc_status".to_owned(), self.rustdoc_status.to_json());
        m.to_json()
    }
}


#[cfg(test)]
mod test {
    extern crate env_logger;
    use super::*;

    #[test]
    #[ignore]
    fn test_start_web_server() {
        // FIXME: This test is doing nothing
        let _ = env_logger::init();
        start_web_server();
    }
}
