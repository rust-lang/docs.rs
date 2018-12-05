//! Web interface of cratesfyi


pub mod page;

/// ctry! (cratesfyitry) is extremely similar to try! and itry!
/// except it returns an error page response instead of plain Err.
macro_rules! ctry {
    ($result:expr) => (match $result {
        Ok(v) => v,
        Err(e) => {
            return super::page::Page::new(format!("{:?}", e)).title("An error has occured")
                .set_status(::iron::status::BadRequest).to_resp("resp");
        }
    })
}

/// cexpect will check an option and if it's not Some
/// it will return an error page response
macro_rules! cexpect {
    ($option:expr) => (match $option {
        Some(v) => v,
        None => {
            return super::page::Page::new("Resource not found".to_owned())
                .title("An error has occured")
                .set_status(::iron::status::BadRequest).to_resp("resp");
        }
    })
}

/// Gets an extension from Request
macro_rules! extension {
    ($req:expr, $ext:ty) => (
        cexpect!($req.extensions.get::<$ext>())
        )
}

mod rustdoc;
mod releases;
mod crate_details;
mod source;
mod pool;
mod file;
mod builds;
mod error;
mod sitemap;

use std::env;
use std::error::Error;
use std::time::Duration;
use std::path::PathBuf;
use iron::prelude::*;
use iron::{Handler, status};
use iron::headers::{CacheControl, CacheDirective, ContentType};
use router::{Router, NoRoute};
use staticfile::Static;
use handlebars_iron::{HandlebarsEngine, DirectorySource};
use time;
use postgres::Connection;
use semver::{Version, VersionReq};
use rustc_serialize::json::{Json, ToJson};
use std::collections::BTreeMap;

/// Duration of static files for staticfile and DatabaseFileHandler (in seconds)
const STATIC_FILE_CACHE_DURATION: u64 = 60 * 60 * 24 * 30 * 12;   // 12 months
const STYLE_CSS: &'static str = include_str!(concat!(env!("OUT_DIR"), "/style.css"));
const OPENSEARCH_XML: &'static [u8] = include_bytes!("opensearch.xml");


struct CratesfyiHandler {
    shared_resource_handler: Box<Handler>,
    router_handler: Box<Handler>,
    database_file_handler: Box<Handler>,
    static_handler: Box<Handler>,
}


impl CratesfyiHandler {
    fn chain<H: Handler>(base: H) -> Chain {
        // TODO: Use DocBuilderOptions for paths
        let mut hbse = HandlebarsEngine::new();
        hbse.add(Box::new(DirectorySource::new("./templates", ".hbs")));

        // load templates
        if let Err(e) = hbse.reload() {
            panic!("Failed to load handlebar templates: {}", e.description());
        }

        let mut chain = Chain::new(base);
        chain.link_before(pool::Pool::new());
        chain.link_after(hbse);
        chain
    }

    pub fn new() -> CratesfyiHandler {
        let mut router = Router::new();
        router.get("/", releases::home_page, "index");
        router.get("/style.css", style_css_handler, "style_css");
        router.get("/about", sitemap::about_handler, "about");
        router.get("/robots.txt", sitemap::robots_txt_handler, "robots_txt");
        router.get("/sitemap.xml", sitemap::sitemap_handler, "sitemap_xml");
        router.get("/opensearch.xml", opensearch_xml_handler, "opensearch_xml");
        router.get("/releases", releases::releases_handler, "releases");
        router.get("/releases/feed",
                   releases::releases_feed_handler,
                   "releases_feed");
        router.get("/releases/recent/:page",
                   releases::releases_handler,
                   "releases_recent_page");
        router.get("/releases/stars", releases::stars_handler, "releases_stars");
        router.get("/releases/stars/:page",
                   releases::stars_handler,
                   "releases_stars_page");
        router.get("/releases/:author",
                   releases::author_handler,
                   "releases_author");
        router.get("/releases/:author/:page",
                   releases::author_handler,
                   "releases_author_page");
        router.get("/releases/activity",
                   releases::activity_handler,
                   "releases_activity");
        router.get("/releases/search",
                   releases::search_handler,
                   "releases_search");
        router.get("/releases/queue",
                   releases::build_queue_handler,
                   "releases_queue");
        router.get("/crate/:name",
                   crate_details::crate_details_handler,
                   "crate_name");
        router.get("/crate/:name/",
                   crate_details::crate_details_handler,
                   "crate_name_");
        router.get("/crate/:name/:version",
                   crate_details::crate_details_handler,
                   "crate_name_version");
        router.get("/crate/:name/:version/",
                   crate_details::crate_details_handler,
                   "crate_name_version_");
        router.get("/crate/:name/:version/builds",
                   builds::build_list_handler,
                   "crate_name_version_builds");
        router.get("/crate/:name/:version/builds.json",
                   builds::build_list_handler,
                   "crate_name_version_builds_json");
        router.get("/crate/:name/:version/builds/:id",
                   builds::build_list_handler,
                   "crate_name_version_builds_id");
        router.get("/crate/:name/:version/source/",
                   source::source_browser_handler,
                   "crate_name_version_source");
        router.get("/crate/:name/:version/source/*",
                   source::source_browser_handler,
                   "crate_name_version_source_");
        router.get("/:crate", rustdoc::rustdoc_redirector_handler, "crate");
        router.get("/:crate/", rustdoc::rustdoc_redirector_handler, "crate_");
        router.get("/:crate/badge.svg", rustdoc::badge_handler, "crate_badge");
        router.get("/:crate/:version",
                   rustdoc::rustdoc_redirector_handler,
                   "crate_version");
        router.get("/:crate/:version/",
                   rustdoc::rustdoc_redirector_handler,
                   "crate_version_");
        router.get("/:crate/:version/search-index.js",
                   rustdoc::rustdoc_html_server_handler,
                   "crate_version_search_index_js");
        router.get("/:crate/:version/settings.html",
                   rustdoc::rustdoc_html_server_handler,
                   "crate_version_settings_html");
        router.get("/:crate/:version/all.html",
                   rustdoc::rustdoc_html_server_handler,
                   "crate_version_all_html");
        router.get("/:crate/:version/aliases.js",
                   rustdoc::rustdoc_html_server_handler,
                   "crate_version_aliases_js");
        router.get("/:crate/:version/:target",
                   rustdoc::rustdoc_redirector_handler,
                   "crate_version_target");
        router.get("/:crate/:version/:target/",
                   rustdoc::rustdoc_html_server_handler,
                   "crate_version_target_");
        router.get("/:crate/:version/:target/*.html",
                   rustdoc::rustdoc_html_server_handler,
                   "crate_version_target_html");

        let shared_resources = Self::chain(rustdoc::SharedResourceHandler);
        let router_chain = Self::chain(router);
        let prefix = PathBuf::from(env::var("CRATESFYI_PREFIX").unwrap()).join("public_html");
        let static_handler = Static::new(prefix)
            .cache(Duration::from_secs(STATIC_FILE_CACHE_DURATION));

        CratesfyiHandler {
            shared_resource_handler: Box::new(shared_resources),
            router_handler: Box::new(router_chain),
            database_file_handler: Box::new(file::DatabaseFileHandler),
            static_handler: Box::new(static_handler),
        }
    }
}


impl Handler for CratesfyiHandler {
    fn handle(&self, req: &mut Request) -> IronResult<Response> {
        // try serving shared rustdoc resources first, then router, then db/static file handler
        // return 404 if none of them return Ok
        self.shared_resource_handler
            .handle(req)
            .or_else(|e| {
                self.router_handler.handle(req).or(Err(e))
            })
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
                let err = if let Some(err) = e.error.downcast::<error::Nope>() {
                    *err
                } else if e.error.downcast::<NoRoute>().is_some() {
                    error::Nope::ResourceNotFound
                } else {
                    panic!("all cratesfyi errors should be of type Nope");
                };
                Self::chain(err).handle(req)
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
        })
        .map(|v| if v == "newest" || v == "latest" { "*".to_owned() } else { v })
        .unwrap_or("*".to_string());

    let versions = {
        let mut versions = Vec::new();
        let rows = conn.query("SELECT versions FROM crates WHERE name = $1", &[&name]).unwrap();
        if rows.len() == 0 {
            return None;
        }
        let versions_json: Json = rows.get(0).get(0);
        for version in versions_json.as_array().unwrap() {
            let version: String = version.as_string().unwrap().to_owned();
            versions.push(version);
        }

        versions
    };

    // first check for exact match
    // we can't expect users to use semver in query
    for version in &versions {
        if version == &req_version {
            return Some(version.clone());
        }
    }

    // Now try to match with semver
    let req_sem_ver = match VersionReq::parse(&req_version) {
        Ok(v) => v,
        Err(_) => return None,
    };

    // we need to sort versions first
    let versions_sem = {
        let mut versions_sem: Vec<Version> = Vec::new();

        for version in &versions {
            // in theory a crate must always have a semver compatible version
            // but check result just in case
            let version = match Version::parse(&version) {
                Ok(v) => v,
                Err(_) => return None,
            };
            versions_sem.push(version);
        }

        versions_sem.sort();
        versions_sem.reverse();
        versions_sem
    };

    // semver is acting weird for '*' (any) range if a crate only have pre-release versions
    // return first version if requested version is '*'
    if req_version == "*" && !versions_sem.is_empty() {
        return Some(format!("{}", versions_sem[0]));
    }

    for version in &versions_sem {
        if req_sem_ver.matches(&version) {
            return Some(format!("{}", version));
        }
    }

    None
}





/// Wrapper around the Markdown parser and renderer to render markdown
fn render_markdown(text: &str) -> String {
    use comrak::{markdown_to_html, ComrakOptions};

    let options = {
        let mut options = ComrakOptions::default();
        options.ext_superscript = true;
        options.ext_table = true;
        options.ext_autolink = true;
        options.ext_tasklist = true;
        options.ext_strikethrough = true;
        options
    };

    markdown_to_html(text, &options)
}



/// Returns latest version if required version is not the latest
/// req_version must be an exact version
fn latest_version(versions_json: &Vec<String>, req_version: &str) -> Option<String> {
    let req_version = match Version::parse(req_version) {
        Ok(v) => v,
        Err(_) => return None,
    };
    let versions = {
        let mut versions: Vec<Version> = Vec::new();
        for version in versions_json {
            let version = match Version::parse(&version) {
                Ok(v) => v,
                Err(_) => return None,
            };
            versions.push(version);
        }

        versions.sort();
        versions.reverse();
        versions
    };

    if req_version != versions[0] {
        for i in 1..versions.len() {
            if req_version == versions[i]  {
                return Some(format!("{}", versions[0]))
            }
        }
    }
    None
}



/// Starts cratesfyi web server
pub fn start_web_server(sock_addr: Option<&str>) {
    let cratesfyi = CratesfyiHandler::new();
    Iron::new(cratesfyi).http(sock_addr.unwrap_or("localhost:3000")).unwrap();
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



fn style_css_handler(_: &mut Request) -> IronResult<Response> {
    let mut response = Response::with((status::Ok, STYLE_CSS));
    let cache = vec![CacheDirective::Public,
                     CacheDirective::MaxAge(STATIC_FILE_CACHE_DURATION as u32)];
    response.headers.set(ContentType("text/css".parse().unwrap()));
    response.headers.set(CacheControl(cache));
    Ok(response)
}


fn opensearch_xml_handler(_: &mut Request) -> IronResult<Response> {
    let mut response = Response::with((status::Ok, OPENSEARCH_XML));
    let cache = vec![CacheDirective::Public,
                     CacheDirective::MaxAge(STATIC_FILE_CACHE_DURATION as u32)];
    response.headers.set(ContentType("application/opensearchdescription+xml".parse().unwrap()));
    response.headers.set(CacheControl(cache));
    Ok(response)
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
                   &[&name, &version])
            .unwrap() {

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
        let _ = env_logger::try_init();
        start_web_server(None);
    }

    #[test]
    fn test_latest_version() {
        let versions = vec!["1.0.0".to_string(),
                            "1.1.0".to_string(),
                            "0.9.0".to_string(),
                            "0.9.1".to_string()];
        assert_eq!(latest_version(&versions, "1.1.0"), None);
        assert_eq!(latest_version(&versions, "1.0.0"), Some("1.1.0".to_owned()));
        assert_eq!(latest_version(&versions, "0.9.0"), Some("1.1.0".to_owned()));
        assert_eq!(latest_version(&versions, "invalidversion"), None);
    }
}
