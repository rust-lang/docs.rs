//! Web interface of cratesfyi


pub mod page;

/// ctry! (cratesfyitry) is extremely similar to try! and itry!
/// except it returns an error page response instead of plain Err.
macro_rules! ctry {
    ($result:expr) => (match $result {
        Ok(v) => v,
        Err(e) => {
            return $crate::web::page::Page::new(format!("{:?}", e)).title("An error has occured")
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
            return $crate::web::page::Page::new("Resource not found".to_owned())
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
mod routes;
pub mod metrics;

use std::{env, fmt};
use std::error::Error;
use std::time::Duration;
use std::path::PathBuf;
use std::net::SocketAddr;
use iron::prelude::*;
use iron::{self, Listening, Handler, Url, status};
use iron::headers::{Expires, HttpDate, CacheControl, CacheDirective, ContentType};
use iron::modifiers::Redirect;
use router::NoRoute;
use staticfile::Static;
use handlebars_iron::{HandlebarsEngine, DirectorySource};
use time;
use postgres::Connection;
use semver::{Version, VersionReq};
use rustc_serialize::json::{Json, ToJson};
use std::collections::BTreeMap;
use self::pool::Pool;

#[cfg(test)]
use std::sync::{Arc, Mutex};

/// Duration of static files for staticfile and DatabaseFileHandler (in seconds)
const STATIC_FILE_CACHE_DURATION: u64 = 60 * 60 * 24 * 30 * 12;   // 12 months
const STYLE_CSS: &'static str = include_str!(concat!(env!("OUT_DIR"), "/style.css"));
const MENU_JS: &'static str = include_str!(concat!(env!("OUT_DIR"), "/menu.js"));
const OPENSEARCH_XML: &'static [u8] = include_bytes!("opensearch.xml");

const DEFAULT_BIND: &str = "0.0.0.0:3000";

type PoolFactoryFn = dyn Fn() -> Pool + Send + Sync;
type PoolFactory = Box<PoolFactoryFn>;

struct CratesfyiHandler {
    shared_resource_handler: Box<dyn Handler>,
    router_handler: Box<dyn Handler>,
    database_file_handler: Box<dyn Handler>,
    static_handler: Box<dyn Handler>,
    pool_factory: PoolFactory,
}


impl CratesfyiHandler {
    fn chain<H: Handler>(pool_factory: &PoolFactoryFn, base: H) -> Chain {
        // TODO: Use DocBuilderOptions for paths
        let mut hbse = HandlebarsEngine::new();
        hbse.add(Box::new(DirectorySource::new("./templates", ".hbs")));

        // load templates
        if let Err(e) = hbse.reload() {
            panic!("Failed to load handlebar templates: {}", e.description());
        }

        let mut chain = Chain::new(base);
        chain.link_before(pool_factory());
        chain.link_after(hbse);
        chain
    }

    pub fn new(pool_factory: PoolFactory) -> CratesfyiHandler {
        let routes = routes::build_routes();
        let blacklisted_prefixes = routes.page_prefixes();

        let shared_resources = Self::chain(&pool_factory, rustdoc::SharedResourceHandler);
        let router_chain = Self::chain(&pool_factory, routes.iron_router());
        let prefix = PathBuf::from(env::var("CRATESFYI_PREFIX").unwrap()).join("public_html");
        let static_handler = Static::new(prefix)
            .cache(Duration::from_secs(STATIC_FILE_CACHE_DURATION));

        CratesfyiHandler {
            shared_resource_handler: Box::new(shared_resources),
            router_handler: Box::new(router_chain),
            database_file_handler: Box::new(routes::BlockBlacklistedPrefixes::new(
                blacklisted_prefixes,
                Box::new(file::DatabaseFileHandler),
            )),
            static_handler: Box::new(static_handler),
            pool_factory,
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
                let err = if let Some(err) = e.error.downcast::<error::Nope>() {
                    *err
                } else if e.error.downcast::<NoRoute>().is_some() {
                    error::Nope::ResourceNotFound
                } else {
                    panic!("all cratesfyi errors should be of type Nope");
                };

                if let error::Nope::ResourceNotFound = err {
                    // print the path of the URL that triggered a 404 error
                    struct DebugPath<'a>(&'a iron::Url);
                    impl<'a> fmt::Display for DebugPath<'a> {
                        fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
                            for path_elem in self.0.path() {
                                write!(f, "/{}", path_elem)?;
                            }

                            if let Some(query) = self.0.query() {
                                write!(f, "?{}", query)?;
                            }

                            if let Some(hash) = self.0.fragment() {
                                write!(f, "#{}", hash)?;
                            }

                            Ok(())
                        }
                    }

                    debug!("Path not found: {}", DebugPath(&req.url));
                }


                Self::chain(&self.pool_factory, err).handle(req)
            })
    }
}

/// Represents the possible results of attempting to load a version requirement.
#[derive(Debug, Clone, PartialEq, Eq)]
enum MatchVersion {
    /// `match_version` was given an exact version, which matched a saved crate version.
    Exact(String),
    /// `match_version` was given a semver version requirement, which matched the given saved crate
    /// version.
    Semver(String),
    /// `match_version` was given a version requirement which did not match any saved crate
    /// versions.
    None,
}

impl MatchVersion {
    /// Convert this `MatchVersion` into an `Option`, discarding whether the matched version came
    /// from an exact version number or a semver requirement.
    pub fn into_option(self) -> Option<String> {
        match self {
            MatchVersion::Exact(v) | MatchVersion::Semver(v) => Some(v),
            MatchVersion::None => None,
        }
    }
}

/// Checks the database for crate releases that match the given name and version.
///
/// `version` may be an exact version number or loose semver version requirement. The return value
/// will indicate whether the given version exactly matched a version number from the database.
fn match_version(conn: &Connection, name: &str, version: Option<&str>) -> MatchVersion {

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

    let versions: Vec<String> = {
        let query = "SELECT version
            FROM releases INNER JOIN crates ON releases.crate_id = crates.id
            WHERE name = $1 AND yanked = false";
        let rows = conn.query(query, &[&name]).unwrap();
        if rows.len() == 0 {
            return MatchVersion::None;
        }
        rows.iter().map(|row| row.get(0)).collect()
    };

    // first check for exact match
    // we can't expect users to use semver in query
    for version in &versions {
        if version == &req_version {
            return MatchVersion::Exact(version.clone());
        }
    }

    // Now try to match with semver
    let req_sem_ver = match VersionReq::parse(&req_version) {
        Ok(v) => v,
        Err(_) => return MatchVersion::None,
    };

    // we need to sort versions first
    let versions_sem = {
        let mut versions_sem: Vec<Version> = Vec::new();

        for version in &versions {
            // in theory a crate must always have a semver compatible version
            // but check result just in case
            let version = match Version::parse(&version) {
                Ok(v) => v,
                Err(_) => return MatchVersion::None,
            };
            versions_sem.push(version);
        }

        versions_sem.sort();
        versions_sem.reverse();
        versions_sem
    };

    for version in &versions_sem {
        if req_sem_ver.matches(&version) {
            return MatchVersion::Semver(format!("{}", version));
        }
    }

    // semver is acting weird for '*' (any) range if a crate only have pre-release versions
    // return first version if requested version is '*'
    if req_version == "*" && !versions_sem.is_empty() {
        return MatchVersion::Semver(format!("{}", versions_sem[0]));
    }

    MatchVersion::None
}





/// Wrapper around the Markdown parser and renderer to render markdown
fn render_markdown(text: &str) -> String {
    use comrak::{markdown_to_html, ComrakOptions};

    let options = {
        let mut options = ComrakOptions::default();
        options.safe = true;
        options.ext_superscript = true;
        options.ext_table = true;
        options.ext_autolink = true;
        options.ext_tasklist = true;
        options.ext_strikethrough = true;
        options
    };

    markdown_to_html(text, &options)
}



pub struct Server {
    inner: Listening,
}

impl Server {
    pub fn start(addr: Option<&str>) -> Self {
        let server = Self::start_inner(addr.unwrap_or(DEFAULT_BIND), Box::new(|| Pool::new()));
        info!("Running docs.rs web server on http://{}", server.addr());
        server
    }

    #[cfg(test)]
    pub(crate) fn start_test(conn: Arc<Mutex<Connection>>) -> Self {
        Self::start_inner("127.0.0.1:0", Box::new(move || Pool::new_simple(conn.clone())))
    }

    fn start_inner(addr: &str, pool_factory: PoolFactory) -> Self {
        // poke all the metrics counters to instantiate and register them
        metrics::TOTAL_BUILDS.inc_by(0);
        metrics::SUCCESSFUL_BUILDS.inc_by(0);
        metrics::FAILED_BUILDS.inc_by(0);
        metrics::NON_LIBRARY_BUILDS.inc_by(0);
        metrics::UPLOADED_FILES_TOTAL.inc_by(0);

        let cratesfyi = CratesfyiHandler::new(pool_factory);
        let inner = Iron::new(cratesfyi).http(addr)
            .unwrap_or_else(|_| panic!("Failed to bind to socket on {}", addr));

        Server { inner }
    }

    pub(crate) fn addr(&self) -> SocketAddr {
        self.inner.socket
    }

    /// Iron is bugged, and it never closes the server even when the listener is dropped. To
    /// avoid never-ending tests this method forgets about the server, leaking it and allowing the
    /// program to end.
    ///
    /// The OS will then close all the dangling servers once the process exits.
    ///
    /// https://docs.rs/iron/0.5/iron/struct.Listening.html#method.close
    #[cfg(test)]
    pub(crate) fn leak(self) {
        std::mem::forget(self.inner);
    }
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

/// Creates a `Response` which redirects to the given path on the scheme/host/port from the given
/// `Request`.
fn redirect(url: Url) -> Response {
    let mut resp = Response::with((status::Found, Redirect(url)));
    resp.headers.set(Expires(HttpDate(time::now())));

    resp
}

pub fn redirect_base(req: &Request) -> String {
    // Try to get the scheme from CloudFront first, and then from iron
    let scheme = req.headers
        .get_raw("cloudfront-forwarded-proto")
        .and_then(|values| values.get(0))
        .and_then(|value| std::str::from_utf8(value).ok())
        .filter(|proto| *proto == "http" || *proto == "https")
        .unwrap_or_else(|| req.url.scheme());

    // Only include the port if it's needed
    let port = req.url.port();
    if port == 80 {
        format!("{}://{}", scheme, req.url.host())
    } else {
        format!("{}://{}:{}", scheme, req.url.host(), port)
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

fn menu_js_handler(_: &mut Request) -> IronResult<Response> {
    let mut response = Response::with((status::Ok, MENU_JS));
    let cache = vec![CacheDirective::Public,
                     CacheDirective::MaxAge(STATIC_FILE_CACHE_DURATION as u32)];
    response.headers.set(ContentType("application/javascript".parse().unwrap()));
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

fn ico_handler(req: &mut Request) -> IronResult<Response> {
    if let Some(&"favicon.ico") = req.url.path().last() {
        // if we're looking for exactly "favicon.ico", we need to defer to the handler that loads
        // from `public_html`, so return a 404 here to make the main handler carry on
        Err(IronError::new(error::Nope::ResourceNotFound, status::NotFound))
    } else {
        // if we're looking for something like "favicon-20190317-1.35.0-nightly-c82834e2b.ico",
        // redirect to the plain one so that the above branch can trigger with the correct filename
        let url = ctry!(Url::parse(&format!("{}/favicon.ico", redirect_base(req))[..]));

        Ok(redirect(url))
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
    pub default_target: String,
}


impl MetaData {
    pub fn from_crate(conn: &Connection, name: &str, version: &str) -> Option<MetaData> {
        for row in &conn.query("SELECT crates.name,
                                       releases.version,
                                       releases.description,
                                       releases.target_name,
                                       releases.rustdoc_status,
                                       releases.default_target
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
                default_target: row.get(5),
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
        m.insert("default_target".to_owned(), self.default_target.to_json());
        m.to_json()
    }
}


#[cfg(test)]
mod test {
    use crate::test::*;
    use web::{match_version, MatchVersion};

    fn release(version: &str, db: &TestDatabase) -> i32 {
        db.fake_release().name("foo").version(version).create().unwrap()
    }
    fn version(v: Option<&str>, db: &TestDatabase) -> MatchVersion {
        match_version(&db.conn(), "foo", v)
    }

    #[test]
    fn test_index_returns_success() {
        wrapper(|env| {
            let web = env.frontend();
            assert!(web.get("/").send()?.status().is_success());
            Ok(())
        });
    }

    #[test]
    // https://github.com/rust-lang/docs.rs/issues/223
    fn prereleases_are_not_considered_for_semver() {
        wrapper(|env| {
            let db = env.db();
            let version = |v| version(v, db);
            let release = |v| release(v, db);

            release("0.3.1-pre");
            assert_eq!(version(Some("*")), MatchVersion::Semver("0.3.1-pre".into()));

            release("0.3.1-alpha");
            assert_eq!(version(Some("0.3.1-alpha")), MatchVersion::Exact("0.3.1-alpha".into()));

            release("0.3.0");
            let three = MatchVersion::Semver("0.3.0".into());
            assert_eq!(version(None), three);
            // same thing but with "*"
            assert_eq!(version(Some("*")), three);
            // make sure exact matches still work
            assert_eq!(version(Some("0.3.0")), MatchVersion::Exact("0.3.0".into()));

            Ok(())
        });
    }

    #[test]
    // https://github.com/rust-lang/docs.rs/issues/221
    fn yanked_crates_are_not_considered() {
        wrapper(|env| {
            let db = env.db();

            let release_id = release("0.3.0", db);
            let query = "UPDATE releases SET yanked = true WHERE id = $1 AND version = '0.3.0'";
            db.conn().query(query, &[&release_id]).unwrap();
            assert_eq!(version(None, db), MatchVersion::None);
            assert_eq!(version(Some("0.3"), db), MatchVersion::None);

            release("0.1.0+4.1", db);
            assert_eq!(version(Some("0.1.0+4.1"), db), MatchVersion::Exact("0.1.0+4.1".into()));
            assert_eq!(version(None, db), MatchVersion::Semver("0.1.0+4.1".into()));

            Ok(())
        });
    }

    #[test]
    // vaguely related to https://github.com/rust-lang/docs.rs/issues/395
    fn metadata_has_no_effect() {
        wrapper(|env| {
            let db = env.db();

            release("0.1.0+4.1", db);
            release("0.1.1", db);
            assert_eq!(version(None, db), MatchVersion::Semver("0.1.1".into()));

            release("0.5.1+zstd.1.4.4", db);
            assert_eq!(version(None, db), MatchVersion::Semver("0.5.1+zstd.1.4.4".into()));
            assert_eq!(version(Some("0.5"), db), MatchVersion::Semver("0.5.1+zstd.1.4.4".into()));
            assert_eq!(version(Some("0.5.1+zstd.1.4.4"), db), MatchVersion::Exact("0.5.1+zstd.1.4.4".into()));

            Ok(())
        });
    }
}
