//! rustdoc handler


use super::pool::Pool;
use super::file::File;
use super::{latest_version, redirect_base};
use super::crate_details::CrateDetails;
use iron::prelude::*;
use iron::{status, Url};
use iron::modifiers::Redirect;
use router::Router;
use super::{match_version, MatchVersion};
use super::error::Nope;
use super::page::Page;
use rustc_serialize::json::{Json, ToJson};
use std::collections::BTreeMap;
use iron::headers::{Expires, HttpDate, CacheControl, CacheDirective};
use time;
use iron::Handler;
use utils;


#[derive(Debug)]
struct RustdocPage {
    pub head: String,
    pub body: String,
    pub body_class: String,
    pub name: String,
    pub full: String,
    pub version: String,
    pub description: Option<String>,
    pub crate_details: Option<CrateDetails>,
}


impl Default for RustdocPage {
    fn default() -> RustdocPage {
        RustdocPage {
            head: String::new(),
            body: String::new(),
            body_class: String::new(),
            name: String::new(),
            full: String::new(),
            version: String::new(),
            description: None,
            crate_details: None,
        }
    }
}


impl ToJson for RustdocPage {
    fn to_json(&self) -> Json {
        let mut m: BTreeMap<String, Json> = BTreeMap::new();
        m.insert("rustdoc_head".to_string(), self.head.to_json());
        m.insert("rustdoc_body".to_string(), self.body.to_json());
        m.insert("rustdoc_body_class".to_string(), self.body_class.to_json());
        m.insert("rustdoc_full".to_string(), self.full.to_json());
        m.insert("rustdoc_status".to_string(), true.to_json());
        m.insert("name".to_string(), self.name.to_json());
        m.insert("version".to_string(), self.version.to_json());
        m.insert("description".to_string(), self.description.to_json());
        m.insert("crate_details".to_string(), self.crate_details.to_json());
        m.to_json()
    }
}


/// Handler called for `/:crate` and `/:crate/:version` URLs. Automatically redirects to the docs
/// or crate details page based on whether the given crate version was successfully built.
pub fn rustdoc_redirector_handler(req: &mut Request) -> IronResult<Response> {

    fn redirect_to_doc(req: &Request,
                       name: &str,
                       vers: &str,
                       target_name: &str)
                       -> IronResult<Response> {
        let url = ctry!(Url::parse(&format!("{}/{}/{}/{}/",
                                            redirect_base(req),
                                            name,
                                            vers,
                                            target_name)[..]));
        let mut resp = Response::with((status::Found, Redirect(url)));
        resp.headers.set(Expires(HttpDate(time::now())));

        Ok(resp)
    }

    fn redirect_to_crate(req: &Request,
                         name: &str,
                         vers: &str)
                         -> IronResult<Response> {
        let url = ctry!(Url::parse(&format!("{}/crate/{}/{}",
                                            redirect_base(req),
                                            name,
                                            vers)[..]));

        let mut resp = Response::with((status::Found, Redirect(url)));
        resp.headers.set(Expires(HttpDate(time::now())));

        Ok(resp)
    }

    // this unwrap is safe because iron urls are always able to use `path_segments`
    // i'm using this instead of `req.url.path()` to avoid allocating the Vec, and also to avoid
    // keeping the borrow alive into the return statement
    if req.url.as_ref().path_segments().unwrap().last().map_or(false, |s| s.ends_with(".js")) {
        // javascript files should be handled by the file server instead of erroneously
        // redirecting to the crate root page
        if req.url.as_ref().path_segments().unwrap().count() > 2 {
            // this URL is actually from a crate-internal path, serve it there instead
            return rustdoc_html_server_handler(req);
        } else {
            let path = req.url.path();
            let path = path.join("/");
            let conn = extension!(req, Pool);
            match File::from_path(&conn, &path) {
                Some(f) => return Ok(f.serve()),
                None => return Err(IronError::new(Nope::ResourceNotFound, status::NotFound)),
            }
        }
    } else if req.url.as_ref().path_segments().unwrap().last().map_or(false, |s| s.ends_with(".ico")) {
        // route .ico files into their dedicated handler so that docs.rs's favicon is always
        // displayed
        return super::ico_handler(req);
    }

    let router = extension!(req, Router);
    // this handler should never called without crate pattern
    let crate_name = cexpect!(router.find("crate"));
    let req_version = router.find("version");

    let conn = extension!(req, Pool);

    // it doesn't matter if the version that was given was exact or not, since we're redirecting
    // anyway
    let version = match match_version(&conn, &crate_name, req_version).into_option() {
        Some(v) => v,
        None => return Err(IronError::new(Nope::CrateNotFound, status::NotFound)),
    };

    // get target name and whether it has docs
    // FIXME: This is a bit inefficient but allowing us to use less code in general
    let (target_name, has_docs): (String, bool) = {
        let rows = ctry!(conn.query("SELECT target_name, rustdoc_status
                                     FROM releases
                                     INNER JOIN crates ON crates.id = releases.crate_id
                                     WHERE crates.name = $1 AND releases.version = $2",
                                    &[&crate_name, &version]));

        (rows.get(0).get(0), rows.get(0).get(1))
    };

    if has_docs {
        redirect_to_doc(req, &crate_name, &version, &target_name)
    } else {
        redirect_to_crate(req, &crate_name, &version)
    }
}


/// Serves documentation generated by rustdoc.
///
/// This includes all HTML files for an individual crate, as well as the `search-index.js`, which is
/// also crate-specific.
pub fn rustdoc_html_server_handler(req: &mut Request) -> IronResult<Response> {

    let router = extension!(req, Router);
    let name = router.find("crate").unwrap_or("").to_string();
    let url_version = router.find("version");
    let version; // pre-declaring it to enforce drop order relative to `req_path`
    let conn = extension!(req, Pool);

    let mut req_path = req.url.path();

    // remove name and version from path
    for _ in 0..2 {
        req_path.remove(0);
    }

    version = match match_version(&conn, &name, url_version) {
        MatchVersion::Exact(v) => v,
        MatchVersion::Semver(v) => {
            // to prevent cloudfront caching the wrong artifacts on URLs with loose semver
            // versions, redirect the browser to the returned version instead of loading it
            // immediately
            let url = ctry!(Url::parse(&format!("{}/{}/{}/{}",
                                                redirect_base(req),
                                                name,
                                                v,
                                                req_path.join("/"))[..]));
            return Ok(super::redirect(url));
        }
        MatchVersion::None => return Err(IronError::new(Nope::ResourceNotFound, status::NotFound)),
    };

    // docs have "rustdoc" prefix in database
    req_path.insert(0, "rustdoc");

    // add crate name and version
    req_path.insert(1, &name);
    req_path.insert(2, &version);

    let path = {
        let mut path = req_path.join("/");
        if path.ends_with("/") {
            path.push_str("index.html");
            req_path.push("index.html");
        }
        path
    };

    let file = match File::from_path(&conn, &path) {
        Some(f) => f,
        None => return Err(IronError::new(Nope::ResourceNotFound, status::NotFound)),
    };

    // serve file directly if it's not html
    if !path.ends_with(".html") {
        return Ok(file.serve());
    }

    let mut content = RustdocPage::default();

    let file_content = ctry!(String::from_utf8(file.content));

    let (head, body, mut body_class) = ctry!(utils::extract_head_and_body(&file_content));
    content.head = head;
    content.body = body;

    if body_class.is_empty() {
        body_class = "rustdoc container-rustdoc".to_string();
    } else {
        // rustdoc adds its own "rustdoc" class to the body
        body_class.push_str(" container-rustdoc");
    }
    content.body_class = body_class;

    content.full = file_content;
    let crate_details = cexpect!(CrateDetails::new(&conn, &name, &version));
    let latest_version = latest_version(&crate_details.versions, &version);

    content.crate_details = Some(crate_details);

    Page::new(content)
        .set_true("show_package_navigation")
        .set_true("package_navigation_documentation_tab")
        .set_true("package_navigation_show_platforms_tab")
        .set_bool("is_latest_version", latest_version.is_none())
        .set("latest_version", &latest_version.unwrap_or(String::new()))
        .to_resp("rustdoc")
}



pub fn badge_handler(req: &mut Request) -> IronResult<Response> {
    use iron::headers::ContentType;
    use params::{Params, Value};
    use badge::{Badge, BadgeOptions};

    let version = {
        let params = ctry!(req.get_ref::<Params>());
        match params.find(&["version"]) {
            Some(&Value::String(ref version)) => version.clone(),
            _ => "*".to_owned(),
        }
    };

    let name = cexpect!(extension!(req, Router).find("crate"));
    let conn = extension!(req, Pool);

    let options = match match_version(&conn, &name, Some(&version)) {
        MatchVersion::Exact(version) => {
            let rows = ctry!(conn.query("SELECT rustdoc_status
                                         FROM releases
                                         INNER JOIN crates ON crates.id = releases.crate_id
                                         WHERE crates.name = $1 AND releases.version = $2",
                                        &[&name, &version]));
            if rows.len() > 0 && rows.get(0).get(0) {
                BadgeOptions {
                    subject: "docs".to_owned(),
                    status: version,
                    color: "#4d76ae".to_owned(),
                }
            } else {
                BadgeOptions {
                    subject: "docs".to_owned(),
                    status: version,
                    color: "#e05d44".to_owned(),
                }
            }
        }
        MatchVersion::Semver(version) => {
            let url = ctry!(Url::parse(&format!("{}/{}/badge.svg?version={}",
                                                redirect_base(req),
                                                name,
                                                version)[..]));

            return Ok(super::redirect(url));
        }
        MatchVersion::None => {
            BadgeOptions {
                subject: "docs".to_owned(),
                status: "no builds".to_owned(),
                color: "#e05d44".to_owned(),
            }
        }
    };

    let mut resp = Response::with((status::Ok, ctry!(Badge::new(options)).to_svg()));
    resp.headers.set(ContentType("image/svg+xml".parse().unwrap()));
    resp.headers.set(Expires(HttpDate(time::now())));
    resp.headers.set(CacheControl(vec![CacheDirective::NoCache,
                                       CacheDirective::NoStore,
                                       CacheDirective::MustRevalidate]));
    Ok(resp)
}

/// Serves shared web resources used by rustdoc-generated documentation.
///
/// This includes common `css` and `js` files that only change when the compiler is updated, but are
/// otherwise the same for all crates documented with that compiler. Those have a custom handler to
/// deduplicate them and save space.
pub struct SharedResourceHandler;

impl Handler for SharedResourceHandler {
    fn handle(&self, req: &mut Request) -> IronResult<Response> {
        let path = req.url.path();
        let filename = path.last().unwrap();  // unwrap is fine: vector is non-empty
        let suffix = filename.split('.').last().unwrap();  // unwrap is fine: split always works
        if ["js", "css", "woff", "svg"].contains(&suffix) {
            let conn = extension!(req, Pool);

            if let Some(file) = File::from_path(conn, filename) {
                return Ok(file.serve());
            }
        }

        // Just always return a 404 here - the main handler will then try the other handlers
        Err(IronError::new(Nope::ResourceNotFound, status::NotFound))
    }
}
