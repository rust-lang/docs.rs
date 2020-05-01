//! rustdoc handler

use super::crate_details::CrateDetails;
use super::error::Nope;
use super::file::File;
use super::page::Page;
use super::pool::Pool;
use super::redirect_base;
use super::{match_version, MatchSemver};
use crate::utils;
use iron::headers::{CacheControl, CacheDirective, Expires, HttpDate};
use iron::modifiers::Redirect;
use iron::prelude::*;
use iron::Handler;
use iron::{status, Url};
use postgres::Connection;
use router::Router;
use rustc_serialize::json::{Json, ToJson};
use std::collections::BTreeMap;

#[derive(Debug, Default)]
struct RustdocPage {
    head: String,
    body: String,
    body_class: String,
    name: String,
    full: String,
    version: String,
    description: Option<String>,
    crate_details: Option<CrateDetails>,
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

#[derive(Clone)]
pub struct RustLangRedirector {
    url: Url,
}

impl RustLangRedirector {
    pub fn new(target: &'static str) -> Self {
        let url = url::Url::parse("https://doc.rust-lang.org/stable/")
            .expect("failed to parse rust-lang.org base URL")
            .join(target)
            .expect("failed to append crate name to rust-lang.org base URL");
        let url = Url::from_generic_url(url).expect("failed to convert url::Url to iron::Url");
        Self { url }
    }
}

impl iron::Handler for RustLangRedirector {
    fn handle(&self, _req: &mut Request) -> IronResult<Response> {
        Ok(Response::with((status::Found, Redirect(self.url.clone()))))
    }
}

/// Handler called for `/:crate` and `/:crate/:version` URLs. Automatically redirects to the docs
/// or crate details page based on whether the given crate version was successfully built.
pub fn rustdoc_redirector_handler(req: &mut Request) -> IronResult<Response> {
    use url::percent_encoding::percent_decode;

    fn redirect_to_doc(
        req: &Request,
        name: &str,
        vers: &str,
        target: Option<&str>,
        target_name: &str,
    ) -> IronResult<Response> {
        let mut url_str = if let Some(target) = target {
            format!(
                "{}/{}/{}/{}/{}/",
                redirect_base(req),
                name,
                vers,
                target,
                target_name
            )
        } else {
            format!("{}/{}/{}/{}/", redirect_base(req), name, vers, target_name)
        };
        if let Some(query) = req.url.query() {
            url_str.push('?');
            url_str.push_str(query);
        }
        let url = ctry!(Url::parse(&url_str[..]));
        let mut resp = Response::with((status::Found, Redirect(url)));
        resp.headers.set(Expires(HttpDate(time::now())));

        Ok(resp)
    }

    fn redirect_to_crate(req: &Request, name: &str, vers: &str) -> IronResult<Response> {
        let url = ctry!(Url::parse(
            &format!("{}/crate/{}/{}", redirect_base(req), name, vers)[..]
        ));

        let mut resp = Response::with((status::Found, Redirect(url)));
        resp.headers.set(Expires(HttpDate(time::now())));

        Ok(resp)
    }

    // this unwrap is safe because iron urls are always able to use `path_segments`
    // i'm using this instead of `req.url.path()` to avoid allocating the Vec, and also to avoid
    // keeping the borrow alive into the return statement
    if req
        .url
        .as_ref()
        .path_segments()
        .unwrap()
        .last()
        .map_or(false, |s| s.ends_with(".js"))
    {
        // javascript files should be handled by the file server instead of erroneously
        // redirecting to the crate root page
        if req.url.as_ref().path_segments().unwrap().count() > 2 {
            // this URL is actually from a crate-internal path, serve it there instead
            return rustdoc_html_server_handler(req);
        } else {
            let path = req.url.path();
            let path = path.join("/");
            let conn = extension!(req, Pool).get();
            match File::from_path(&conn, &path) {
                Some(f) => return Ok(f.serve()),
                None => return Err(IronError::new(Nope::ResourceNotFound, status::NotFound)),
            }
        }
    } else if req
        .url
        .as_ref()
        .path_segments()
        .unwrap()
        .last()
        .map_or(false, |s| s.ends_with(".ico"))
    {
        // route .ico files into their dedicated handler so that docs.rs's favicon is always
        // displayed
        return super::ico_handler(req);
    }

    let router = extension!(req, Router);
    // this handler should never called without crate pattern
    let crate_name = cexpect!(router.find("crate"));
    let mut crate_name = percent_decode(crate_name.as_bytes())
        .decode_utf8()
        .unwrap_or_else(|_| crate_name.into())
        .into_owned();
    let req_version = router.find("version");
    let mut target = router.find("target");

    let conn = extension!(req, Pool).get();

    // it doesn't matter if the version that was given was exact or not, since we're redirecting
    // anyway
    let (version, id) = match match_version(&conn, &crate_name, req_version) {
        Some(v) => {
            if let Some(new_name) = v.corrected_name {
                // `match_version` checked against -/_ typos, so if we have a name here we should
                // use that instead
                crate_name = new_name;
            }
            v.version.into_parts()
        }
        None => {
            return Err(IronError::new(Nope::CrateNotFound, status::NotFound));
        }
    };

    // get target name and whether it has docs
    // FIXME: This is a bit inefficient but allowing us to use less code in general
    let (target_name, has_docs): (String, bool) = {
        let rows = ctry!(conn.query(
            "SELECT target_name, rustdoc_status
             FROM releases
             WHERE releases.id = $1",
            &[&id]
        ));

        (rows.get(0).get(0), rows.get(0).get(1))
    };

    if target == Some("index.html") || target == Some(&target_name) {
        target = None;
    }

    if has_docs {
        redirect_to_doc(req, &crate_name, &version, target, &target_name)
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
    let conn = extension!(req, Pool).get();

    let mut req_path = req.url.path();

    // remove name and version from path
    for _ in 0..2 {
        req_path.remove(0);
    }

    let redirect = |name: &str, vers: &str, path: &[&str]| -> IronResult<Response> {
        let url = ctry!(Url::parse(
            &format!(
                "{}/{}/{}/{}",
                redirect_base(req),
                name,
                vers,
                path.join("/")
            )[..]
        ));
        Ok(super::redirect(url))
    };

    let corrected_name;
    version = match match_version(&conn, &name, url_version) {
        Some(mv) => {
            corrected_name = mv.corrected_name;

            match mv.version {
                MatchSemver::Exact((v, _)) => v,
                MatchSemver::Semver((v, _)) => {
                    // to prevent cloudfront caching the wrong artifacts on URLs with loose semver
                    // versions, redirect the browser to the returned version instead of loading it
                    // immediately
                    return redirect(&name, &v, &req_path);
                }
            }
        }
        None => return Err(IronError::new(Nope::ResourceNotFound, status::NotFound)),
    };

    if let Some(name) = corrected_name {
        return redirect(&name, &version, &req_path);
    }

    // docs have "rustdoc" prefix in database
    req_path.insert(0, "rustdoc");

    // add crate name and version
    req_path.insert(1, &name);
    req_path.insert(2, &version);

    let crate_details = cexpect!(CrateDetails::new(&conn, &name, &version));

    // if visiting the full path to the default target, remove the target from the path
    // expects a req_path that looks like `/rustdoc/:crate/:version[/:target]/.*`
    if req_path[3] == crate_details.metadata.default_target {
        return redirect(&name, &version, &req_path[4..]);
    }

    let mut path = {
        let mut path = req_path.join("/");
        if path.ends_with('/') {
            req_path.pop(); // get rid of empty string
            path.push_str("index.html");
            req_path.push("index.html");
        }
        path
    };

    let file = match File::from_path(&conn, &path) {
        Some(f) => f,
        None => {
            // If it fails, we try again with /index.html at the end
            path.push_str("/index.html");
            req_path.push("index.html");
            match File::from_path(&conn, &path) {
                Some(f) => f,
                None => return Err(IronError::new(Nope::ResourceNotFound, status::NotFound)),
            }
        }
    };

    let req_path = req_path;

    // serve file directly if it's not html
    if !path.ends_with(".html") {
        return Ok(file.serve());
    }

    let mut content = RustdocPage::default();

    let file_content = ctry!(String::from_utf8(file.0.content));

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

    let latest_version = crate_details.latest_version().to_owned();
    let is_latest_version = latest_version == version;

    let path_in_latest = if !is_latest_version {
        let mut latest_path = req_path.clone();
        latest_path[2] = &latest_version;
        path_for_version(&latest_path, &crate_details.doc_targets, &conn)
    } else {
        Default::default()
    };

    // The path within this crate version's rustdoc output
    let inner_path = {
        let mut inner_path = req_path.clone();
        // Drop the `rustdoc/:crate/:version[/:platform]` prefix
        inner_path.drain(..3);
        if inner_path.len() > 1 && crate_details.doc_targets.iter().any(|s| s == inner_path[0]) {
            inner_path.remove(0);
        }
        inner_path.join("/")
    };

    content.crate_details = Some(crate_details);

    Page::new(content)
        .set_true("show_package_navigation")
        .set_true("package_navigation_documentation_tab")
        .set_true("package_navigation_show_platforms_tab")
        .set_bool("is_latest_version", is_latest_version)
        .set("path_in_latest", &path_in_latest)
        .set("latest_version", &latest_version)
        .set("inner_path", &inner_path)
        .to_resp("rustdoc")
}

/// Checks whether the given path exists.
/// The crate's `target_name` is used to confirm whether a platform triple is part of the path.
///
/// Note that path is overloaded in this context to mean both the path of a URL
/// and the file path of a static file in the DB.
///
/// `req_path` is assumed to have the following format:
/// `rustdoc/crate/version[/platform]/module/[kind.name.html|index.html]`
///
/// Returns a path that can be appended to `/crate/version/` to create a complete URL.
fn path_for_version(req_path: &[&str], known_platforms: &[String], conn: &Connection) -> String {
    // Simple case: page exists in the latest version, so just change the version number
    if File::from_path(&conn, &req_path.join("/")).is_some() {
        // NOTE: this adds 'index.html' if it wasn't there before
        return req_path[3..].join("/");
    }
    // this page doesn't exist in the latest version
    let search_item = if *req_path.last().unwrap() == "index.html" {
        // this is a module
        req_path[req_path.len() - 2]
    } else {
        // this is an item
        req_path
            .last()
            .unwrap()
            .split('.')
            .nth(1)
            .expect("paths should be of the form <kind>.<name>.html")
    };
    // check if req_path[3] is the platform choice or the name of the crate
    let platform = if known_platforms.iter().any(|s| s == req_path[3]) && req_path.len() >= 5 {
        req_path[3]
    } else {
        ""
    };
    format!("{}?search={}", platform, search_item)
}

pub fn target_redirect_handler(req: &mut Request) -> IronResult<Response> {
    let router = extension!(req, Router);
    let name = cexpect!(router.find("name"));
    let version = cexpect!(router.find("version"));

    let conn = extension!(req, Pool).get();
    let base = redirect_base(req);

    let crate_details = cexpect!(CrateDetails::new(&conn, &name, &version));

    //   [crate, :name, :version, target-redirect, :target, *path]
    // is transformed to
    //   [rustdoc, :name, :version, :target?, *path]
    // path might be empty, but target is guaranteed to be there because of the route used
    let file_path = {
        let mut file_path = req.url.path();
        file_path[0] = "rustdoc";
        file_path.remove(3);
        if file_path[3] == crate_details.metadata.default_target {
            file_path.remove(3);
        }
        if let Some(last) = file_path.last_mut() {
            if *last == "" {
                *last = "index.html";
            }
        }
        file_path
    };

    let path = path_for_version(&file_path, &crate_details.doc_targets, &conn);
    let url = format!(
        "{base}/{name}/{version}/{path}",
        base = base,
        name = name,
        version = version,
        path = path
    );

    let url = ctry!(Url::parse(&url));
    let mut resp = Response::with((status::Found, Redirect(url)));
    resp.headers.set(Expires(HttpDate(time::now())));

    Ok(resp)
}

pub fn badge_handler(req: &mut Request) -> IronResult<Response> {
    use badge::{Badge, BadgeOptions};
    use iron::headers::ContentType;
    use params::{Params, Value};

    let version = {
        let params = ctry!(req.get_ref::<Params>());
        match params.find(&["version"]) {
            Some(&Value::String(ref version)) => version.clone(),
            _ => "*".to_owned(),
        }
    };

    let name = cexpect!(extension!(req, Router).find("crate"));
    let conn = extension!(req, Pool).get();

    let options = match match_version(&conn, &name, Some(&version)).and_then(|m| m.assume_exact()) {
        Some(MatchSemver::Exact((version, id))) => {
            let rows = ctry!(conn.query(
                "SELECT rustdoc_status
                 FROM releases
                 WHERE releases.id = $1",
                &[&id]
            ));
            if !rows.is_empty() && rows.get(0).get(0) {
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

        Some(MatchSemver::Semver((version, _))) => {
            let base_url = format!("{}/{}/badge.svg", redirect_base(req), name);
            let url = ctry!(url::Url::parse_with_params(
                &base_url,
                &[("version", version)]
            ));
            let iron_url = ctry!(Url::from_generic_url(url));
            return Ok(super::redirect(iron_url));
        }

        None => BadgeOptions {
            subject: "docs".to_owned(),
            status: "no builds".to_owned(),
            color: "#e05d44".to_owned(),
        },
    };

    let mut resp = Response::with((status::Ok, ctry!(Badge::new(options)).to_svg()));
    resp.headers
        .set(ContentType("image/svg+xml".parse().unwrap()));
    resp.headers.set(Expires(HttpDate(time::now())));
    resp.headers.set(CacheControl(vec![
        CacheDirective::NoCache,
        CacheDirective::NoStore,
        CacheDirective::MustRevalidate,
    ]));
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
        let filename = path.last().unwrap(); // unwrap is fine: vector is non-empty
        let suffix = filename.split('.').last().unwrap(); // unwrap is fine: split always works
        if ["js", "css", "woff", "svg"].contains(&suffix) {
            let conn = extension!(req, Pool).get();

            if let Some(file) = File::from_path(&conn, filename) {
                return Ok(file.serve());
            }
        }

        // Just always return a 404 here - the main handler will then try the other handlers
        Err(IronError::new(Nope::ResourceNotFound, status::NotFound))
    }
}

#[cfg(test)]
mod test {
    use crate::test::*;
    use reqwest::StatusCode;
    use std::{collections::BTreeMap, iter::FromIterator};

    fn try_latest_version_redirect(
        path: &str,
        web: &TestFrontend,
    ) -> Result<Option<String>, failure::Error> {
        use html5ever::tendril::TendrilSink;
        assert_success(path, web)?;
        let data = web.get(path).send()?.text()?;
        let dom = kuchiki::parse_html().one(data);
        if let Some(elem) = dom
            .select("form ul li a.warn")
            .expect("invalid selector")
            .next()
        {
            let link = elem.attributes.borrow().get("href").unwrap().to_string();
            assert_success(&link, web)?;
            Ok(Some(link))
        } else {
            Ok(None)
        }
    }

    fn latest_version_redirect(path: &str, web: &TestFrontend) -> Result<String, failure::Error> {
        try_latest_version_redirect(path, web)
            .and_then(|v| v.ok_or_else(|| failure::format_err!("no redirect found for {}", path)))
    }

    #[test]
    // regression test for https://github.com/rust-lang/docs.rs/issues/552
    fn settings_html() {
        wrapper(|env| {
            let db = env.db();
            // first release works, second fails
            db.fake_release()
                .name("buggy")
                .version("0.1.0")
                .build_result_successful(true)
                .rustdoc_file("settings.html", b"some data")
                .rustdoc_file("directory_1/index.html", b"some data 1")
                .rustdoc_file("directory_2.html/index.html", b"some data 1")
                .rustdoc_file("all.html", b"some data 2")
                .rustdoc_file("directory_3/.gitignore", b"*.ext")
                .rustdoc_file("directory_4/empty_file_no_ext", b"")
                .create()?;
            db.fake_release()
                .name("buggy")
                .version("0.2.0")
                .build_result_successful(false)
                .create()?;
            let web = env.frontend();
            assert_success("/", web)?;
            assert_success("/crate/buggy/0.1.0/", web)?;
            assert_success("/buggy/0.1.0/directory_1/index.html", web)?;
            assert_success("/buggy/0.1.0/directory_2.html/index.html", web)?;
            assert_success("/buggy/0.1.0/directory_3/.gitignore", web)?;
            assert_success("/buggy/0.1.0/settings.html", web)?;
            assert_success("/buggy/0.1.0/all.html", web)?;
            assert_success("/buggy/0.1.0/directory_4/empty_file_no_ext", web)?;
            Ok(())
        });
    }

    #[test]
    fn default_target_redirects_to_base() {
        wrapper(|env| {
            let db = env.db();
            db.fake_release()
                .name("dummy")
                .version("0.1.0")
                .rustdoc_file("dummy/index.html", b"some content")
                .create()?;

            let web = env.frontend();
            // no explicit default-target
            let base = "/dummy/0.1.0/dummy/";
            assert_success(base, web)?;
            assert_redirect("/dummy/0.1.0/x86_64-unknown-linux-gnu/dummy/", base, web)?;

            // set an explicit target that requires cross-compile
            let target = "x86_64-pc-windows-msvc";
            db.fake_release()
                .name("dummy")
                .version("0.2.0")
                .rustdoc_file("dummy/index.html", b"some content")
                .default_target(target)
                .create()?;
            let base = "/dummy/0.2.0/dummy/";
            assert_success(base, web)?;
            assert_redirect("/dummy/0.2.0/x86_64-pc-windows-msvc/dummy/", base, web)?;

            // set an explicit target without cross-compile
            // also check that /:crate/:version/:platform/all.html doesn't panic
            let target = "x86_64-unknown-linux-gnu";
            db.fake_release()
                .name("dummy")
                .version("0.3.0")
                .rustdoc_file("dummy/index.html", b"some content")
                .rustdoc_file("all.html", b"html")
                .default_target(target)
                .create()?;
            let base = "/dummy/0.3.0/dummy/";
            assert_success(base, web)?;
            assert_redirect("/dummy/0.3.0/x86_64-unknown-linux-gnu/dummy/", base, web)?;
            assert_redirect(
                "/dummy/0.3.0/x86_64-unknown-linux-gnu/all.html",
                "/dummy/0.3.0/all.html",
                web,
            )?;
            assert_redirect("/dummy/0.3.0/", base, web)?;
            assert_redirect("/dummy/0.3.0/index.html", base, web)?;
            Ok(())
        });
    }

    #[test]
    fn go_to_latest_version() {
        wrapper(|env| {
            let db = env.db();
            db.fake_release()
                .name("dummy")
                .version("0.1.0")
                .rustdoc_file("dummy/blah/index.html", b"lah")
                .rustdoc_file("dummy/blah/blah.html", b"lah")
                .rustdoc_file("dummy/struct.will-be-deleted.html", b"lah")
                .create()?;
            db.fake_release()
                .name("dummy")
                .version("0.2.0")
                .rustdoc_file("dummy/blah/index.html", b"lah")
                .rustdoc_file("dummy/blah/blah.html", b"lah")
                .create()?;

            let web = env.frontend();

            // check it works at all
            let redirect = latest_version_redirect("/dummy/0.1.0/dummy/", &web)?;
            assert_eq!(redirect, "/dummy/0.2.0/dummy/index.html");

            // check it keeps the subpage
            let redirect = latest_version_redirect("/dummy/0.1.0/dummy/blah/", &web)?;
            assert_eq!(redirect, "/dummy/0.2.0/dummy/blah/index.html");
            let redirect = latest_version_redirect("/dummy/0.1.0/dummy/blah/blah.html", &web)?;
            assert_eq!(redirect, "/dummy/0.2.0/dummy/blah/blah.html");

            // check it searches for removed pages
            let redirect =
                latest_version_redirect("/dummy/0.1.0/dummy/struct.will-be-deleted.html", &web)?;
            // This must be a double redirect to deal with crates that failed to build in the
            // latest version
            assert_eq!(redirect, "/dummy/0.2.0/?search=will-be-deleted");
            assert_redirect(
                &redirect,
                "/dummy/0.2.0/dummy/?search=will-be-deleted",
                &web,
            )?;

            Ok(())
        })
    }

    #[test]
    fn go_to_latest_version_keeps_platform() {
        wrapper(|env| {
            let db = env.db();
            db.fake_release()
                .name("dummy")
                .version("0.1.0")
                .add_platform("x86_64-pc-windows-msvc")
                .rustdoc_file("dummy/struct.Blah.html", b"lah")
                .create()
                .unwrap();
            db.fake_release()
                .name("dummy")
                .version("0.2.0")
                .add_platform("x86_64-pc-windows-msvc")
                .create()
                .unwrap();

            let web = env.frontend();

            let redirect =
                latest_version_redirect("/dummy/0.1.0/x86_64-pc-windows-msvc/dummy", web)?;
            assert_eq!(
                redirect,
                "/dummy/0.2.0/x86_64-pc-windows-msvc/dummy/index.html"
            );

            let redirect =
                latest_version_redirect("/dummy/0.1.0/x86_64-pc-windows-msvc/dummy/", web)?;
            assert_eq!(
                redirect,
                "/dummy/0.2.0/x86_64-pc-windows-msvc/dummy/index.html"
            );

            // With deleted file platform specific redirect also handles search
            let redirect = latest_version_redirect(
                "/dummy/0.1.0/x86_64-pc-windows-msvc/dummy/struct.Blah.html",
                web,
            )?;
            assert_eq!(redirect, "/dummy/0.2.0/x86_64-pc-windows-msvc?search=Blah");
            assert_redirect(
                &redirect,
                "/dummy/0.2.0/x86_64-pc-windows-msvc/dummy/?search=Blah",
                web,
            )?;

            Ok(())
        })
    }

    #[test]
    fn redirect_latest_goes_to_crate_if_build_failed() {
        wrapper(|env| {
            let db = env.db();
            db.fake_release()
                .name("dummy")
                .version("0.1.0")
                .rustdoc_file("dummy/index.html", b"lah")
                .create()
                .unwrap();
            db.fake_release()
                .name("dummy")
                .version("0.2.0")
                .build_result_successful(false)
                .create()
                .unwrap();

            let web = env.frontend();
            let redirect = latest_version_redirect("/dummy/0.1.0/dummy/", web)?;
            assert_eq!(redirect, "/crate/dummy/0.2.0");

            Ok(())
        })
    }

    #[test]
    fn redirect_latest_does_not_go_to_yanked_versions() {
        wrapper(|env| {
            let db = env.db();
            db.fake_release()
                .name("dummy")
                .version("0.1.0")
                .rustdoc_file("dummy/index.html", b"lah")
                .create()
                .unwrap();
            db.fake_release()
                .name("dummy")
                .version("0.2.0")
                .rustdoc_file("dummy/index.html", b"lah")
                .create()
                .unwrap();
            db.fake_release()
                .name("dummy")
                .version("0.2.1")
                .rustdoc_file("dummy/index.html", b"lah")
                .cratesio_data_yanked(true)
                .create()
                .unwrap();

            let web = env.frontend();
            let redirect = latest_version_redirect("/dummy/0.1.0/dummy/", web)?;
            assert_eq!(redirect, "/dummy/0.2.0/dummy/index.html");

            let redirect = latest_version_redirect("/dummy/0.2.1/dummy/", web)?;
            assert_eq!(redirect, "/dummy/0.2.0/dummy/index.html");

            Ok(())
        })
    }

    #[test]
    fn redirect_latest_with_all_yanked() {
        wrapper(|env| {
            let db = env.db();
            db.fake_release()
                .name("dummy")
                .version("0.1.0")
                .rustdoc_file("dummy/index.html", b"lah")
                .cratesio_data_yanked(true)
                .create()
                .unwrap();
            db.fake_release()
                .name("dummy")
                .version("0.2.0")
                .rustdoc_file("dummy/index.html", b"lah")
                .cratesio_data_yanked(true)
                .create()
                .unwrap();
            db.fake_release()
                .name("dummy")
                .version("0.2.1")
                .rustdoc_file("dummy/index.html", b"lah")
                .cratesio_data_yanked(true)
                .create()
                .unwrap();

            let web = env.frontend();
            let redirect = latest_version_redirect("/dummy/0.1.0/dummy/", web)?;
            assert_eq!(redirect, "/dummy/0.2.1/dummy/index.html");

            let redirect = latest_version_redirect("/dummy/0.2.0/dummy/", web)?;
            assert_eq!(redirect, "/dummy/0.2.1/dummy/index.html");

            Ok(())
        })
    }

    #[test]
    fn yanked_release_shows_warning_in_nav() {
        fn has_yanked_warning(path: &str, web: &TestFrontend) -> Result<bool, failure::Error> {
            use html5ever::tendril::TendrilSink;
            assert_success(path, web)?;
            let data = web.get(path).send()?.text()?;
            Ok(kuchiki::parse_html()
                .one(data)
                .select("form > ul > li > .warn")
                .expect("invalid selector")
                .any(|el| el.text_contents().contains("yanked")))
        }

        wrapper(|env| {
            let (db, web) = (env.db(), env.frontend());

            db.fake_release()
                .name("dummy")
                .version("0.1.0")
                .rustdoc_file("dummy/index.html", b"lah")
                .cratesio_data_yanked(true)
                .create()
                .unwrap();

            assert!(has_yanked_warning("/dummy/0.1.0/dummy/", web)?);

            db.fake_release()
                .name("dummy")
                .version("0.2.0")
                .rustdoc_file("dummy/index.html", b"lah")
                .cratesio_data_yanked(true)
                .create()
                .unwrap();

            assert!(has_yanked_warning("/dummy/0.1.0/dummy/", web)?);

            Ok(())
        })
    }

    #[test]
    fn badges_are_urlencoded() {
        wrapper(|env| {
            let db = env.db();
            db.fake_release()
                .name("zstd")
                .version("0.5.1+zstd.1.4.4")
                .create()?;

            let frontend = env.frontend();
            assert_redirect(
                "/zstd/badge.svg",
                "/zstd/badge.svg?version=0.5.1%2Bzstd.1.4.4",
                &frontend,
            )?;
            Ok(())
        })
    }

    #[test]
    fn crate_name_percent_decoded_redirect() {
        wrapper(|env| {
            env.db()
                .fake_release()
                .name("fake-crate")
                .version("0.0.1")
                .rustdoc_file("fake_crate/index.html", b"some content")
                .create()
                .unwrap();

            let web = env.frontend();
            assert_redirect("/fake%2Dcrate", "/fake-crate/0.0.1/fake_crate/", web)?;

            Ok(())
        });
    }

    #[test]
    fn base_redirect_handles_mismatched_separators() {
        wrapper(|env| {
            let db = env.db();

            let rels = [
                ("dummy-dash", "0.1.0"),
                ("dummy-dash", "0.2.0"),
                ("dummy_underscore", "0.1.0"),
                ("dummy_underscore", "0.2.0"),
                ("dummy_mixed-separators", "0.1.0"),
                ("dummy_mixed-separators", "0.2.0"),
            ];

            for (name, version) in &rels {
                db.fake_release()
                    .name(name)
                    .version(version)
                    .rustdoc_file(&(name.replace("-", "_") + "/index.html"), b"")
                    .create()?;
            }

            let web = env.frontend();

            assert_redirect("/dummy_dash", "/dummy-dash/0.2.0/dummy_dash/", web)?;
            assert_redirect("/dummy_dash/*", "/dummy-dash/0.2.0/dummy_dash/", web)?;
            assert_redirect("/dummy_dash/0.1.0", "/dummy-dash/0.1.0/dummy_dash/", web)?;
            assert_redirect(
                "/dummy-underscore",
                "/dummy_underscore/0.2.0/dummy_underscore/",
                web,
            )?;
            assert_redirect(
                "/dummy-underscore/*",
                "/dummy_underscore/0.2.0/dummy_underscore/",
                web,
            )?;
            assert_redirect(
                "/dummy-underscore/0.1.0",
                "/dummy_underscore/0.1.0/dummy_underscore/",
                web,
            )?;
            assert_redirect(
                "/dummy-mixed_separators",
                "/dummy_mixed-separators/0.2.0/dummy_mixed_separators/",
                web,
            )?;
            assert_redirect(
                "/dummy_mixed_separators/*",
                "/dummy_mixed-separators/0.2.0/dummy_mixed_separators/",
                web,
            )?;
            assert_redirect(
                "/dummy-mixed-separators/0.1.0",
                "/dummy_mixed-separators/0.1.0/dummy_mixed_separators/",
                web,
            )?;

            Ok(())
        })
    }

    #[test]
    fn specific_pages_do_not_handle_mismatched_separators() {
        wrapper(|env| {
            let db = env.db();

            db.fake_release()
                .name("dummy-dash")
                .version("0.1.0")
                .rustdoc_file("dummy_dash/index.html", b"")
                .create()?;

            db.fake_release()
                .name("dummy_mixed-separators")
                .version("0.1.0")
                .rustdoc_file("dummy_mixed_separators/index.html", b"")
                .create()?;

            let web = env.frontend();

            assert_success("/dummy-dash/0.1.0/dummy_dash/index.html", web)?;
            assert_success("/crate/dummy_mixed-separators", web)?;

            assert_redirect(
                "/dummy_dash/0.1.0/dummy_dash/index.html",
                "/dummy-dash/0.1.0/dummy_dash/index.html",
                web,
            )?;

            assert_eq!(
                web.get("/crate/dummy_mixed_separators").send()?.status(),
                StatusCode::NOT_FOUND
            );

            Ok(())
        })
    }

    #[test]
    fn nonexistent_crate_404s() {
        wrapper(|env| {
            assert_eq!(
                env.frontend().get("/dummy").send()?.status(),
                StatusCode::NOT_FOUND
            );

            Ok(())
        })
    }

    #[test]
    fn no_target_target_redirect_404s() {
        wrapper(|env| {
            assert_eq!(
                env.frontend()
                    .get("/crate/dummy/0.1.0/target-redirect")
                    .send()?
                    .status(),
                StatusCode::NOT_FOUND
            );

            assert_eq!(
                env.frontend()
                    .get("/crate/dummy/0.1.0/target-redirect/")
                    .send()?
                    .status(),
                StatusCode::NOT_FOUND
            );

            Ok(())
        })
    }

    #[test]
    fn platform_links_go_to_current_path() {
        fn get_platform_links(
            path: &str,
            web: &TestFrontend,
        ) -> Result<Vec<(String, String)>, failure::Error> {
            use html5ever::tendril::TendrilSink;
            assert_success(path, web)?;
            let data = web.get(path).send()?.text()?;
            let dom = kuchiki::parse_html().one(data);
            Ok(dom
                .select(r#"a[aria-label="Platform"] + ul li a"#)
                .expect("invalid selector")
                .map(|el| {
                    let url = el
                        .attributes
                        .borrow()
                        .get("href")
                        .expect("href")
                        .to_string();
                    let name = el.text_contents();
                    (name, url)
                })
                .collect())
        }

        fn assert_platform_links(
            web: &TestFrontend,
            path: &str,
            links: &[(&str, &str)],
        ) -> Result<(), failure::Error> {
            let mut links = BTreeMap::from_iter(links.iter().copied());

            for (platform, link) in get_platform_links(path, web)? {
                assert_redirect(&link, links.remove(platform.as_str()).unwrap(), web)?;
            }

            assert!(links.is_empty());

            Ok(())
        }

        wrapper(|env| {
            let (db, web) = (env.db(), env.frontend());

            // no explicit default-target
            db.fake_release()
                .name("dummy")
                .version("0.1.0")
                .rustdoc_file("dummy/index.html", b"some content")
                .rustdoc_file("dummy/struct.Dummy.html", b"some other content")
                .add_target("x86_64-unknown-linux-gnu")
                .create()?;

            assert_platform_links(
                web,
                "/dummy/0.1.0/dummy/",
                &[("x86_64-unknown-linux-gnu", "/dummy/0.1.0/dummy/index.html")],
            )?;

            assert_platform_links(
                web,
                "/dummy/0.1.0/dummy/index.html",
                &[("x86_64-unknown-linux-gnu", "/dummy/0.1.0/dummy/index.html")],
            )?;

            assert_platform_links(
                web,
                "/dummy/0.1.0/dummy/struct.Dummy.html",
                &[(
                    "x86_64-unknown-linux-gnu",
                    "/dummy/0.1.0/dummy/struct.Dummy.html",
                )],
            )?;

            // set an explicit target that requires cross-compile
            db.fake_release()
                .name("dummy")
                .version("0.2.0")
                .rustdoc_file("dummy/index.html", b"some content")
                .rustdoc_file("dummy/struct.Dummy.html", b"some other content")
                .default_target("x86_64-pc-windows-msvc")
                .create()?;

            assert_platform_links(
                web,
                "/dummy/0.2.0/dummy/",
                &[("x86_64-pc-windows-msvc", "/dummy/0.2.0/dummy/index.html")],
            )?;

            assert_platform_links(
                web,
                "/dummy/0.2.0/dummy/index.html",
                &[("x86_64-pc-windows-msvc", "/dummy/0.2.0/dummy/index.html")],
            )?;

            assert_platform_links(
                web,
                "/dummy/0.2.0/dummy/struct.Dummy.html",
                &[(
                    "x86_64-pc-windows-msvc",
                    "/dummy/0.2.0/dummy/struct.Dummy.html",
                )],
            )?;

            // set an explicit target without cross-compile
            db.fake_release()
                .name("dummy")
                .version("0.3.0")
                .rustdoc_file("dummy/index.html", b"some content")
                .rustdoc_file("dummy/struct.Dummy.html", b"some other content")
                .default_target("x86_64-unknown-linux-gnu")
                .create()?;

            assert_platform_links(
                web,
                "/dummy/0.3.0/dummy/",
                &[("x86_64-unknown-linux-gnu", "/dummy/0.3.0/dummy/index.html")],
            )?;

            assert_platform_links(
                web,
                "/dummy/0.3.0/dummy/index.html",
                &[("x86_64-unknown-linux-gnu", "/dummy/0.3.0/dummy/index.html")],
            )?;

            assert_platform_links(
                web,
                "/dummy/0.3.0/dummy/struct.Dummy.html",
                &[(
                    "x86_64-unknown-linux-gnu",
                    "/dummy/0.3.0/dummy/struct.Dummy.html",
                )],
            )?;

            // multiple targets
            db.fake_release()
                .name("dummy")
                .version("0.4.0")
                .rustdoc_file("settings.html", b"top-level items")
                .rustdoc_file("dummy/index.html", b"some content")
                .rustdoc_file("dummy/struct.Dummy.html", b"some other content")
                .rustdoc_file("dummy/struct.DefaultOnly.html", b"some otter content")
                .rustdoc_file("x86_64-pc-windows-msvc/settings.html", b"top-level items")
                .rustdoc_file("x86_64-pc-windows-msvc/dummy/index.html", b"some content")
                .rustdoc_file(
                    "x86_64-pc-windows-msvc/dummy/struct.Dummy.html",
                    b"some other content",
                )
                .rustdoc_file(
                    "x86_64-pc-windows-msvc/dummy/struct.WindowsOnly.html",
                    b"some otter content",
                )
                .default_target("x86_64-unknown-linux-gnu")
                .add_target("x86_64-pc-windows-msvc")
                .create()?;

            assert_platform_links(
                web,
                "/dummy/0.4.0/settings.html",
                &[
                    (
                        "x86_64-pc-windows-msvc",
                        "/dummy/0.4.0/x86_64-pc-windows-msvc/settings.html",
                    ),
                    ("x86_64-unknown-linux-gnu", "/dummy/0.4.0/settings.html"),
                ],
            )?;

            assert_platform_links(
                web,
                "/dummy/0.4.0/dummy/",
                &[
                    (
                        "x86_64-pc-windows-msvc",
                        "/dummy/0.4.0/x86_64-pc-windows-msvc/dummy/index.html",
                    ),
                    ("x86_64-unknown-linux-gnu", "/dummy/0.4.0/dummy/index.html"),
                ],
            )?;

            assert_platform_links(
                web,
                "/dummy/0.4.0/x86_64-pc-windows-msvc/dummy/index.html",
                &[
                    (
                        "x86_64-pc-windows-msvc",
                        "/dummy/0.4.0/x86_64-pc-windows-msvc/dummy/index.html",
                    ),
                    ("x86_64-unknown-linux-gnu", "/dummy/0.4.0/dummy/index.html"),
                ],
            )?;

            assert_platform_links(
                web,
                "/dummy/0.4.0/dummy/index.html",
                &[
                    (
                        "x86_64-pc-windows-msvc",
                        "/dummy/0.4.0/x86_64-pc-windows-msvc/dummy/index.html",
                    ),
                    ("x86_64-unknown-linux-gnu", "/dummy/0.4.0/dummy/index.html"),
                ],
            )?;

            assert_platform_links(
                web,
                "/dummy/0.4.0/dummy/struct.DefaultOnly.html",
                &[
                    (
                        "x86_64-pc-windows-msvc",
                        "/dummy/0.4.0/x86_64-pc-windows-msvc/dummy/?search=DefaultOnly",
                    ),
                    (
                        "x86_64-unknown-linux-gnu",
                        "/dummy/0.4.0/dummy/struct.DefaultOnly.html",
                    ),
                ],
            )?;

            assert_platform_links(
                web,
                "/dummy/0.4.0/dummy/struct.Dummy.html",
                &[
                    (
                        "x86_64-pc-windows-msvc",
                        "/dummy/0.4.0/x86_64-pc-windows-msvc/dummy/struct.Dummy.html",
                    ),
                    (
                        "x86_64-unknown-linux-gnu",
                        "/dummy/0.4.0/dummy/struct.Dummy.html",
                    ),
                ],
            )?;

            assert_platform_links(
                web,
                "/dummy/0.4.0/x86_64-pc-windows-msvc/dummy/struct.Dummy.html",
                &[
                    (
                        "x86_64-pc-windows-msvc",
                        "/dummy/0.4.0/x86_64-pc-windows-msvc/dummy/struct.Dummy.html",
                    ),
                    (
                        "x86_64-unknown-linux-gnu",
                        "/dummy/0.4.0/dummy/struct.Dummy.html",
                    ),
                ],
            )?;

            assert_platform_links(
                web,
                "/dummy/0.4.0/x86_64-pc-windows-msvc/dummy/struct.WindowsOnly.html",
                &[
                    (
                        "x86_64-pc-windows-msvc",
                        "/dummy/0.4.0/x86_64-pc-windows-msvc/dummy/struct.WindowsOnly.html",
                    ),
                    (
                        "x86_64-unknown-linux-gnu",
                        "/dummy/0.4.0/dummy/?search=WindowsOnly",
                    ),
                ],
            )?;

            Ok(())
        });
    }
}
