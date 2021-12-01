//! rustdoc handler

use crate::{
    db::Pool,
    repositories::RepositoryStatsUpdater,
    utils,
    web::{
        crate_details::CrateDetails, csp::Csp, error::Nope, file::File, match_version,
        metrics::RenderingTimesRecorder, redirect_base, MatchSemver, MetaData,
    },
    Config, Metrics, Storage,
};
use anyhow::{anyhow, Context};
use iron::{
    headers::{CacheControl, CacheDirective},
    url::percent_encoding::percent_decode,
};
use iron::{
    headers::{CacheControl, CacheDirective, Expires, HttpDate},
    modifiers::Redirect,
    status, Handler, IronResult, Request, Response, Url,
};
use lol_html::errors::RewritingError;
use router::Router;
use serde::Serialize;
use std::{fmt::Write, path::Path};

#[derive(Clone)]
pub struct RustLangRedirector {
    url: Url,
}

impl RustLangRedirector {
    pub fn new(version: &str, target: &str) -> Self {
        let url = iron::url::Url::parse(&format!("https://doc.rust-lang.org/{version}/{target}/"))
            .expect("failed to parse rust-lang.org doc URL");
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
    fn redirect_to_doc(
        req: &Request,
        name: &str,
        vers: &str,
        target: Option<&str>,
        target_name: &str,
        path_in_crate: Option<&str>,
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
        } else if let Some(path) = path_in_crate {
            url_str.push_str("?search=");
            url_str.push_str(path);
        }
        let url = ctry!(req, Url::parse(&url_str));
        let (status_code, max_age) = if vers == "latest" {
            (status::MovedPermanently, 86400)
        } else {
            (status::Found, 0)
        };
        let mut resp = Response::with((status_code, Redirect(url)));
        resp.headers
            .set(CacheControl(vec![CacheDirective::MaxAge(max_age)]));
        Ok(resp)
    }

    fn redirect_to_crate(req: &Request, name: &str, vers: &str) -> IronResult<Response> {
        let url = ctry!(
            req,
            Url::parse(&format!("{}/crate/{}/{}", redirect_base(req), name, vers)),
        );

        let mut resp = Response::with((status::Found, Redirect(url)));
        resp.headers.set(Expires(HttpDate(time::now())));

        Ok(resp)
    }

    let metrics = extension!(req, Metrics).clone();
    let mut rendering_time = RenderingTimesRecorder::new(&metrics.rustdoc_redirect_rendering_times);

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
            rendering_time.step("serve JS for crate");
            return rustdoc_html_server_handler(req);
        } else {
            rendering_time.step("serve JS");
            let storage = extension!(req, Storage);
            let config = extension!(req, Config);

            let path = req.url.path();
            let path = path.join("/");
            return match File::from_path(storage, &path, config) {
                Ok(f) => Ok(f.serve()),
                Err(..) => Err(Nope::ResourceNotFound.into()),
            };
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
        rendering_time.step("serve ICO");
        return super::statics::ico_handler(req);
    }

    let router = extension!(req, Router);
    let mut conn = extension!(req, Pool).get()?;

    // this handler should never called without crate pattern
    let crate_name = cexpect!(req, router.find("crate"));
    let crate_name = percent_decode(crate_name.as_bytes())
        .decode_utf8()
        .unwrap_or_else(|_| crate_name.into());
    let (mut crate_name, path_in_crate) = match crate_name.split_once("::") {
        Some((krate, path)) => (krate.to_string(), Some(path.to_string())),
        None => (crate_name.to_string(), None),
    };
    let req_version = router.find("version");
    let mut target = router.find("target");

    // it doesn't matter if the version that was given was exact or not, since we're redirecting
    // anyway
    rendering_time.step("match version");
    let v = match_version(&mut conn, &crate_name, req_version)?;
    if let Some(new_name) = v.corrected_name {
        // `match_version` checked against -/_ typos, so if we have a name here we should
        // use that instead
        crate_name = new_name;
    }
    let (mut version, id) = v.version.into_parts();

    if req_version == None || req_version == Some("latest") {
        version = "latest".to_string()
    }

    // get target name and whether it has docs
    // FIXME: This is a bit inefficient but allowing us to use less code in general
    rendering_time.step("fetch release doc status");
    let (target_name, has_docs): (String, bool) = {
        let rows = ctry!(
            req,
            conn.query(
                "SELECT target_name, rustdoc_status
                 FROM releases
                 WHERE releases.id = $1",
                &[&id]
            ),
        );

        (rows[0].get(0), rows[0].get(1))
    };

    if target == Some("index.html") || target == Some(&target_name) {
        target = None;
    }

    if has_docs {
        rendering_time.step("redirect to doc");
        redirect_to_doc(
            req,
            &crate_name,
            &version,
            target,
            &target_name,
            path_in_crate.as_deref(),
        )
    } else {
        rendering_time.step("redirect to crate");
        redirect_to_crate(req, &crate_name, &version)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
struct RustdocPage {
    latest_path: String,
    canonical_url: String,
    permalink_path: String,
    latest_version: String,
    target: String,
    inner_path: String,
    // true if we are displaying the latest version of the crate, regardless
    // of whether the URL specifies a version number or the string "latest."
    is_latest_version: bool,
    // true if the URL specifies a version using the string "latest."
    is_latest_url: bool,
    is_prerelease: bool,
    krate: CrateDetails,
    metadata: MetaData,
}

impl RustdocPage {
    fn into_response(
        self,
        rustdoc_html: &[u8],
        max_parse_memory: usize,
        req: &mut Request,
        file_path: &str,
    ) -> IronResult<Response> {
        use iron::{headers::ContentType, status::Status};

        let templates = req
            .extensions
            .get::<super::TemplateData>()
            .expect("missing TemplateData from the request extensions");

        let metrics = req
            .extensions
            .get::<crate::Metrics>()
            .expect("missing Metrics from the request extensions");

        let is_latest_url = self.is_latest_url;
        // Build the page of documentation
        let ctx = ctry!(req, tera::Context::from_serialize(self));
        // Extract the head and body of the rustdoc file so that we can insert it into our own html
        // while logging OOM errors from html rewriting
        let html = match utils::rewrite_lol(rustdoc_html, max_parse_memory, ctx, templates) {
            Err(RewritingError::MemoryLimitExceeded(..)) => {
                metrics.html_rewrite_ooms.inc();

                let config = extension!(req, Config);
                let err = anyhow!(
                    "Failed to serve the rustdoc file '{}' because rewriting it surpassed the memory limit of {} bytes",
                    file_path, config.max_parse_memory,
                );

                ctry!(req, Err(err))
            }
            result => ctry!(req, result),
        };

        let mut response = Response::with((Status::Ok, html));
        response.headers.set(ContentType::html());
        if is_latest_url {
            response
                .headers
                .set(CacheControl(vec![CacheDirective::MaxAge(0)]));
        } else {
            response.headers.set(CacheControl(vec![
                CacheDirective::Extension(
                    "stale-while-revalidate".to_string(),
                    Some("2592000".to_string()), // sixty days
                ),
                CacheDirective::MaxAge(600u32), // ten minutes
            ]));
        }
        Ok(response)
    }
}

/// Serves documentation generated by rustdoc.
///
/// This includes all HTML files for an individual crate, as well as the `search-index.js`, which is
/// also crate-specific.
pub fn rustdoc_html_server_handler(req: &mut Request) -> IronResult<Response> {
    let metrics = extension!(req, Metrics).clone();
    let mut rendering_time = RenderingTimesRecorder::new(&metrics.rustdoc_rendering_times);

    // Pages generated by Rustdoc are not ready to be served with a CSP yet.
    req.extensions
        .get_mut::<Csp>()
        .expect("missing CSP")
        .suppress(true);

    // Get the request parameters
    let router = extension!(req, Router);

    // Get the crate name and version from the request
    let (name, url_version) = (
        router.find("crate").unwrap_or("").to_string(),
        router.find("version"),
    );

    let pool = extension!(req, Pool);
    let mut conn = pool.get()?;
    let config = extension!(req, Config);
    let storage = extension!(req, Storage);
    let mut req_path = req.url.path();

    // Remove the name and version from the path
    req_path.drain(..2).for_each(drop);

    // Convenience closure to allow for easy redirection
    let redirect = |name: &str, vers: &str, path: &[&str]| -> IronResult<Response> {
        // Format and parse the redirect url
        let redirect_path = format!(
            "{}/{}/{}/{}",
            redirect_base(req),
            name,
            vers,
            path.join("/")
        );
        let url = ctry!(req, Url::parse(&redirect_path));

        Ok(super::redirect(url))
    };

    rendering_time.step("match version");

    // Check the database for releases with the requested version while doing the following:
    // * If no matching releases are found, return a 404 with the underlying error
    // Then:
    // * If both the name and the version are an exact match, return the version of the crate.
    // * If there is an exact match, but the requested crate name was corrected (dashes vs. underscores), redirect to the corrected name.
    // * If there is a semver (but not exact) match, redirect to the exact version.
    let release_found = match_version(&mut conn, &name, url_version)?;

    let (version, version_or_latest) = match release_found.version {
        MatchSemver::Exact((version, _)) => {
            // Redirect when the requested crate name isn't correct
            if let Some(name) = release_found.corrected_name {
                return redirect(&name, &version, &req_path);
            }

            (version.clone(), version)
        }

        MatchSemver::Latest((version, _)) => {
            // Redirect when the requested crate name isn't correct
            if let Some(name) = release_found.corrected_name {
                return redirect(&name, "latest", &req_path);
            }

            (version, "latest".to_string())
        }

        // Redirect when the requested version isn't correct
        MatchSemver::Semver((v, _)) => {
            // to prevent cloudfront caching the wrong artifacts on URLs with loose semver
            // versions, redirect the browser to the returned version instead of loading it
            // immediately
            return redirect(&name, &v, &req_path);
        }
    };

    let updater = extension!(req, RepositoryStatsUpdater);

    rendering_time.step("crate details");

    // Get the crate's details from the database
    // NOTE: we know this crate must exist because we just checked it above (or else `match_version` is buggy)
    let krate = cexpect!(
        req,
        ctry!(
            req,
            CrateDetails::new(
                &mut *conn,
                &name,
                &version,
                &version_or_latest,
                Some(updater)
            )
        )
    );

    // if visiting the full path to the default target, remove the target from the path
    // expects a req_path that looks like `[/:target]/.*`
    if req_path.first().copied() == Some(&krate.metadata.default_target) {
        return redirect(&name, &version_or_latest, &req_path[1..]);
    }

    // Create the path to access the file from
    let mut path = req_path.join("/");
    if path.ends_with('/') {
        req_path.pop(); // get rid of empty string
        path.push_str("index.html");
        req_path.push("index.html");
    }
    let mut path = ctry!(req, percent_decode(path.as_bytes()).decode_utf8());

    // Attempt to load the file from the database
    let blob = match storage.fetch_rustdoc_file(
        &name,
        &version,
        &path,
        krate.archive_storage,
        &mut rendering_time,
    ) {
        Ok(file) => file,
        Err(err) => {
            if !matches!(err.downcast_ref(), Some(Nope::ResourceNotFound))
                && !matches!(err.downcast_ref(), Some(crate::storage::PathNotFoundError))
            {
                log::debug!("got error serving {}: {}", path, err);
            }
            // If it fails, we try again with /index.html at the end
            path.to_mut().push_str("/index.html");
            req_path.push("index.html");

            return if ctry!(
                req,
                storage.rustdoc_file_exists(&name, &version, &path, krate.archive_storage)
            ) {
                redirect(&name, &version_or_latest, &req_path)
            } else if req_path.first().map_or(false, |p| p.contains('-')) {
                // This is a target, not a module; it may not have been built.
                // Redirect to the default target and show a search page instead of a hard 404.
                redirect(
                    &format!("/crate/{}", name),
                    &format!("{}/target-redirect", version),
                    &req_path,
                )
            } else {
                Err(Nope::ResourceNotFound.into())
            };
        }
    };

    // Serve non-html files directly
    if !path.ends_with(".html") {
        rendering_time.step("serve asset");

        return Ok(File(blob).serve());
    }

    rendering_time.step("find latest path");

    let latest_release = krate.latest_release();

    // Get the latest version of the crate
    let latest_version = latest_release.version.to_string();
    let is_latest_version = latest_version == version;
    let is_prerelease = !(semver::Version::parse(&version)
        .with_context(|| {
            format!(
                "invalid semver in database for crate {}: {}",
                name, &version
            )
        })
        // should be impossible unless there is a semver incompatible version in the db
        // Note that there is a redirect earlier for semver matches to the exact version
        .map_err(|err| {
            utils::report_error(&err);
            Nope::InternalServerError
        })?
        .pre
        .is_empty());

    // The path within this crate version's rustdoc output
    let (target, inner_path) = {
        let mut inner_path = req_path.clone();

        let target = if inner_path.len() > 1
            && krate
                .metadata
                .doc_targets
                .iter()
                .any(|s| s == inner_path[0])
        {
            inner_path.remove(0)
        } else {
            ""
        };

        (target, inner_path.join("/"))
    };

    // Find the path of the latest version for the `Go to latest` and `Permalink` links
    let target_redirect = if latest_release.build_status {
        let target = if target.is_empty() {
            &krate.metadata.default_target
        } else {
            target
        };
        format!("/target-redirect/{}/{}", target, inner_path)
    } else {
        "".to_string()
    };

    let query_string = if let Some(query) = req.url.query() {
        format!("?{}", query)
    } else {
        "".to_string()
    };

    let permalink_path = format!(
        "/{}/{}/{}{}",
        name, latest_version, inner_path, query_string
    );

    let latest_path = format!("/crate/{}/latest{}{}", name, target_redirect, query_string);

    // Set the canonical URL for search engines to the `/latest/` page on docs.rs.
    // Note: The URL this points to may not exist. For instance, if we're rendering
    // `struct Foo` in version 0.1.0 of a crate, and version 0.2.0 of that crate removes
    // `struct Foo`, this will point at a 404. That's fine: search engines will crawl
    // the target and will not canonicalize to a URL that doesn't exist.
    // Don't include index.html in the canonical URL.
    let canonical_url = format!(
        "https://docs.rs/{}/latest/{}",
        name,
        inner_path.replace("index.html", "")
    );

    metrics
        .recently_accessed_releases
        .record(krate.crate_id, krate.release_id, target);

    let target = if target.is_empty() {
        String::new()
    } else {
        format!("{}/", target)
    };

    rendering_time.step("rewrite html");
    RustdocPage {
        latest_path,
        canonical_url,
        permalink_path,
        latest_version,
        target,
        inner_path,
        is_latest_version,
        is_latest_url: version_or_latest == "latest",
        is_prerelease,
        metadata: krate.metadata.clone(),
        krate,
    }
    .into_response(&blob.content, config.max_parse_memory, req, &path)
}

/// Checks whether the given path exists.
/// The crate's `target_name` is used to confirm whether a platform triple is part of the path.
///
/// Note that path is overloaded in this context to mean both the path of a URL
/// and the file path of a static file in the DB.
///
/// `file_path` is assumed to have the following format:
/// `[/platform]/module/[kind.name.html|index.html]`
///
/// Returns a path that can be appended to `/crate/version/` to create a complete URL.
fn path_for_version(file_path: &[&str], crate_details: &CrateDetails) -> String {
    // check if req_path[3] is the platform choice or the name of the crate
    // Note we don't require the platform to have a trailing slash.
    let platform = if crate_details
        .metadata
        .doc_targets
        .iter()
        .any(|s| s == file_path[0])
        && !file_path.is_empty()
    {
        file_path[0]
    } else {
        ""
    };
    let is_source_view = if platform.is_empty() {
        // /{name}/{version}/src/{crate}/index.html
        file_path.first().copied() == Some("src")
    } else {
        // /{name}/{version}/{platform}/src/{crate}/index.html
        file_path.get(1).copied() == Some("src")
    };
    // this page doesn't exist in the latest version
    let last_component = *file_path.last().unwrap();
    let search_item = if last_component == "index.html" {
        // this is a module
        file_path.get(file_path.len() - 2).copied()
    // no trailing slash; no one should be redirected here but we handle it gracefully anyway
    } else if last_component == platform {
        // nothing to search for
        None
    } else if !is_source_view {
        // this is an item
        last_component.split('.').nth(1)
    } else {
        // if this is a Rust source file, try searching for the module;
        // else, don't try searching at all, we don't know how to find it
        last_component.strip_suffix(".rs.html")
    };
    let target_name = &crate_details.target_name;
    let mut result = if platform.is_empty() {
        format!("{target_name}/")
    } else {
        format!("{platform}/{target_name}/")
    };
    if let Some(search) = search_item {
        write!(result, "?search={search}").unwrap();
    }
    result
}

pub fn target_redirect_handler(req: &mut Request) -> IronResult<Response> {
    let router = extension!(req, Router);
    let name = cexpect!(req, router.find("name"));
    let version = cexpect!(req, router.find("version"));

    let pool = extension!(req, Pool);
    let mut conn = pool.get()?;
    let storage = extension!(req, Storage);
    let base = redirect_base(req);
    let updater = extension!(req, RepositoryStatsUpdater);

    let release_found = match_version(&mut conn, name, Some(version))?;

    let (version, version_or_latest) = match release_found.version {
        MatchSemver::Exact((version, _)) => (version.clone(), version),
        MatchSemver::Latest((version, _)) => (version, "latest".to_string()),
        // semver matching not supported here
        MatchSemver::Semver(_) => return Err(Nope::VersionNotFound.into()),
    };

    let crate_details = match ctry!(
        req,
        CrateDetails::new(
            &mut *conn,
            name,
            &version,
            &version_or_latest,
            Some(updater)
        )
    ) {
        Some(krate) => krate,
        None => return Err(Nope::VersionNotFound.into()),
    };

    //   [crate, :name, :version, target-redirect, :target, *path]
    // is transformed to
    //   [:target?, *path]
    // path might be empty, but target is guaranteed to be there because of the route used
    let file_path = {
        let mut path = req.url.path();
        path.drain(0..4); // crate, name, version, target-redirect

        if path[0] == crate_details.metadata.default_target {
            path.remove(0);
        }
        // if it ends with a `/`, we add `index.html`.
        if let Some(last @ &mut "") = path.last_mut() {
            *last = "index.html";
        }
        path
    };

    let path = if ctry!(
        req,
        storage.rustdoc_file_exists(
            name,
            &version,
            &file_path.join("/"),
            crate_details.archive_storage
        )
    ) {
        // Simple case: page exists in the other target & version, so just change these
        file_path.join("/")
    } else {
        path_for_version(&file_path, &crate_details)
    };

    let url = format!("{base}/{name}/{version_or_latest}/{path}");

    let url = ctry!(req, Url::parse(&url));
    let mut resp = Response::with((status::Found, Redirect(url)));
    resp.headers.set(Expires(HttpDate(time::now())));

    Ok(resp)
}

pub fn badge_handler(req: &mut Request) -> IronResult<Response> {
    let version = req
        .url
        .as_ref()
        .query_pairs()
        .find(|(key, _)| key == "version");
    let version = version
        .as_ref()
        .map(|(_, version)| version.as_ref())
        .unwrap_or("latest");
    let name = cexpect!(req, extension!(req, Router).find("crate"));
    let url = format!("https://img.shields.io/docsrs/{}/{}", name, version);
    let url = ctry!(req, Url::parse(&url));
    Ok(Response::with((status::MovedPermanently, Redirect(url))))
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
        if let Some(extension) = Path::new(filename).extension() {
            if ["js", "css", "woff", "woff2", "svg", "png"]
                .iter()
                .any(|s| *s == extension)
            {
                let storage = extension!(req, Storage);
                let config = extension!(req, Config);

                if let Ok(file) = File::from_path(storage, filename, config) {
                    return Ok(file.serve());
                }
            }
        }

        // Just always return a 404 here - the main handler will then try the other handlers
        Err(Nope::ResourceNotFound.into())
    }
}

#[cfg(test)]
mod test {
    use crate::test::*;
    use anyhow::Context;
    use kuchiki::traits::TendrilSink;
    use reqwest::{blocking::ClientBuilder, redirect, StatusCode};
    use std::collections::BTreeMap;
    use test_case::test_case;

    fn try_latest_version_redirect(
        path: &str,
        web: &TestFrontend,
    ) -> Result<Option<String>, anyhow::Error> {
        assert_success(path, web)?;
        let data = web.get(path).send()?.text()?;
        log::info!("fetched path {} and got content {}\nhelp: if this is missing the header, remember to add <html><head></head><body></body></html>", path, data);
        let dom = kuchiki::parse_html().one(data);

        if let Some(elem) = dom
            .select("form > ul > li > a.warn")
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

    fn latest_version_redirect(path: &str, web: &TestFrontend) -> Result<String, anyhow::Error> {
        try_latest_version_redirect(path, web)?
            .with_context(|| anyhow::anyhow!("no redirect found for {}", path))
    }

    #[test_case(true)]
    #[test_case(false)]
    // regression test for https://github.com/rust-lang/docs.rs/issues/552
    fn settings_html(archive_storage: bool) {
        wrapper(|env| {
            // first release works, second fails
            env.fake_release()
                .name("buggy")
                .version("0.1.0")
                .archive_storage(archive_storage)
                .rustdoc_file("settings.html")
                .rustdoc_file("directory_1/index.html")
                .rustdoc_file("directory_2.html/index.html")
                .rustdoc_file("all.html")
                .rustdoc_file("directory_3/.gitignore")
                .rustdoc_file("directory_4/empty_file_no_ext")
                .create()?;
            env.fake_release()
                .name("buggy")
                .version("0.2.0")
                .archive_storage(archive_storage)
                .build_result_failed()
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

    #[test_case(true)]
    #[test_case(false)]
    fn default_target_redirects_to_base(archive_storage: bool) {
        wrapper(|env| {
            env.fake_release()
                .name("dummy")
                .version("0.1.0")
                .archive_storage(archive_storage)
                .rustdoc_file("dummy/index.html")
                .create()?;

            let web = env.frontend();
            // no explicit default-target
            let base = "/dummy/0.1.0/dummy/";
            assert_success(base, web)?;
            assert_redirect("/dummy/0.1.0/x86_64-unknown-linux-gnu/dummy/", base, web)?;

            assert_success("/dummy/latest/dummy/", web)?;

            // set an explicit target that requires cross-compile
            let target = "x86_64-pc-windows-msvc";
            env.fake_release()
                .name("dummy")
                .version("0.2.0")
                .archive_storage(archive_storage)
                .rustdoc_file("dummy/index.html")
                .default_target(target)
                .create()?;
            let base = "/dummy/0.2.0/dummy/";
            assert_success(base, web)?;
            assert_redirect("/dummy/0.2.0/x86_64-pc-windows-msvc/dummy/", base, web)?;

            // set an explicit target without cross-compile
            // also check that /:crate/:version/:platform/all.html doesn't panic
            let target = "x86_64-unknown-linux-gnu";
            env.fake_release()
                .name("dummy")
                .version("0.3.0")
                .archive_storage(archive_storage)
                .rustdoc_file("dummy/index.html")
                .rustdoc_file("all.html")
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
    fn latest_url() {
        wrapper(|env| {
            env.fake_release()
                .name("dummy")
                .version("0.1.0")
                .archive_storage(true)
                .rustdoc_file("dummy/index.html")
                .create()?;

            let resp = env.frontend().get("/dummy/latest/dummy/").send()?;
            assert!(resp.url().as_str().ends_with("/dummy/latest/dummy/"));
            let body = String::from_utf8(resp.bytes().unwrap().to_vec()).unwrap();
            assert!(body.contains("<a href=\"/crate/dummy/latest/source/\""));
            assert!(body.contains("<a href=\"/crate/dummy/latest\""));
            assert!(body.contains("<a href=\"/dummy/0.1.0/dummy/index.html\""));
            Ok(())
        })
    }

    #[test]
    fn cache_headers() {
        wrapper(|env| {
            env.fake_release()
                .name("dummy")
                .version("0.1.0")
                .archive_storage(true)
                .rustdoc_file("dummy/index.html")
                .create()?;

            let resp = env.frontend().get("/dummy/latest/dummy/").send()?;
            assert_eq!(resp.headers().get("Cache-Control").unwrap(), &"max-age=0");

            let resp = env.frontend().get("/dummy/0.1.0/dummy/").send()?;
            assert_eq!(
                resp.headers().get("Cache-Control").unwrap(),
                &"stale-while-revalidate=2592000, max-age=600"
            );
            Ok(())
        })
    }

    #[test_case(true)]
    #[test_case(false)]
    fn go_to_latest_version(archive_storage: bool) {
        wrapper(|env| {
            env.fake_release()
                .name("dummy")
                .version("0.1.0")
                .archive_storage(archive_storage)
                .rustdoc_file("dummy/blah/index.html")
                .rustdoc_file("dummy/blah/blah.html")
                .rustdoc_file("dummy/struct.will-be-deleted.html")
                .create()?;
            env.fake_release()
                .name("dummy")
                .version("0.2.0")
                .archive_storage(archive_storage)
                .rustdoc_file("dummy/blah/index.html")
                .rustdoc_file("dummy/blah/blah.html")
                .create()?;

            let web = env.frontend();

            // check it works at all
            let redirect = latest_version_redirect("/dummy/0.1.0/dummy/", web)?;
            assert_eq!(
                redirect,
                "/crate/dummy/latest/target-redirect/x86_64-unknown-linux-gnu/dummy/index.html"
            );

            // check it keeps the subpage
            let redirect = latest_version_redirect("/dummy/0.1.0/dummy/blah/", web)?;
            assert_eq!(
                redirect,
                "/crate/dummy/latest/target-redirect/x86_64-unknown-linux-gnu/dummy/blah/index.html"
            );
            let redirect = latest_version_redirect("/dummy/0.1.0/dummy/blah/blah.html", web)?;
            assert_eq!(
                redirect,
                "/crate/dummy/latest/target-redirect/x86_64-unknown-linux-gnu/dummy/blah/blah.html"
            );

            // check it also works for deleted pages
            let redirect =
                latest_version_redirect("/dummy/0.1.0/dummy/struct.will-be-deleted.html", web)?;
            assert_eq!(redirect, "/crate/dummy/latest/target-redirect/x86_64-unknown-linux-gnu/dummy/struct.will-be-deleted.html");

            Ok(())
        })
    }

    #[test_case(true)]
    #[test_case(false)]
    fn go_to_latest_version_keeps_platform(archive_storage: bool) {
        wrapper(|env| {
            env.fake_release()
                .name("dummy")
                .version("0.1.0")
                .archive_storage(archive_storage)
                .add_platform("x86_64-pc-windows-msvc")
                .rustdoc_file("dummy/struct.Blah.html")
                .create()?;
            env.fake_release()
                .name("dummy")
                .version("0.2.0")
                .archive_storage(archive_storage)
                .add_platform("x86_64-pc-windows-msvc")
                .create()?;

            let web = env.frontend();

            let redirect =
                latest_version_redirect("/dummy/0.1.0/x86_64-pc-windows-msvc/dummy", web)?;
            assert_eq!(
                redirect,
                "/crate/dummy/latest/target-redirect/x86_64-pc-windows-msvc/dummy/index.html"
            );

            let redirect =
                latest_version_redirect("/dummy/0.1.0/x86_64-pc-windows-msvc/dummy/", web)?;
            assert_eq!(
                redirect,
                "/crate/dummy/latest/target-redirect/x86_64-pc-windows-msvc/dummy/index.html"
            );

            let redirect = latest_version_redirect(
                "/dummy/0.1.0/x86_64-pc-windows-msvc/dummy/struct.Blah.html",
                web,
            )?;
            assert_eq!(
                redirect,
                "/crate/dummy/latest/target-redirect/x86_64-pc-windows-msvc/dummy/struct.Blah.html"
            );

            Ok(())
        })
    }

    #[test_case(true)]
    #[test_case(false)]
    fn redirect_latest_goes_to_crate_if_build_failed(archive_storage: bool) {
        wrapper(|env| {
            env.fake_release()
                .name("dummy")
                .version("0.1.0")
                .archive_storage(archive_storage)
                .rustdoc_file("dummy/index.html")
                .create()?;
            env.fake_release()
                .name("dummy")
                .version("0.2.0")
                .archive_storage(archive_storage)
                .build_result_failed()
                .create()?;

            let web = env.frontend();
            let redirect = latest_version_redirect("/dummy/0.1.0/dummy/", web)?;
            assert_eq!(redirect, "/crate/dummy/latest");

            Ok(())
        })
    }

    #[test_case(true)]
    #[test_case(false)]
    fn redirect_latest_does_not_go_to_yanked_versions(archive_storage: bool) {
        wrapper(|env| {
            env.fake_release()
                .name("dummy")
                .version("0.1.0")
                .archive_storage(archive_storage)
                .rustdoc_file("dummy/index.html")
                .create()?;
            env.fake_release()
                .name("dummy")
                .version("0.2.0")
                .archive_storage(archive_storage)
                .rustdoc_file("dummy/index.html")
                .create()?;
            env.fake_release()
                .name("dummy")
                .version("0.2.1")
                .archive_storage(archive_storage)
                .rustdoc_file("dummy/index.html")
                .yanked(true)
                .create()?;

            let web = env.frontend();
            let redirect = latest_version_redirect("/dummy/0.1.0/dummy/", web)?;
            assert_eq!(
                redirect,
                "/crate/dummy/latest/target-redirect/x86_64-unknown-linux-gnu/dummy/index.html"
            );

            let redirect = latest_version_redirect("/dummy/0.2.1/dummy/", web)?;
            assert_eq!(
                redirect,
                "/crate/dummy/latest/target-redirect/x86_64-unknown-linux-gnu/dummy/index.html"
            );

            Ok(())
        })
    }

    #[test_case(true)]
    #[test_case(false)]
    fn yanked_release_shows_warning_in_nav(archive_storage: bool) {
        fn has_yanked_warning(path: &str, web: &TestFrontend) -> Result<bool, anyhow::Error> {
            assert_success(path, web)?;
            let data = web.get(path).send()?.text()?;
            Ok(kuchiki::parse_html()
                .one(data)
                .select("form > ul > li > .warn")
                .expect("invalid selector")
                .any(|el| el.text_contents().contains("yanked")))
        }

        wrapper(|env| {
            let web = env.frontend();

            env.fake_release()
                .name("dummy")
                .version("0.1.0")
                .archive_storage(archive_storage)
                .rustdoc_file("dummy/index.html")
                .yanked(true)
                .create()?;

            assert!(has_yanked_warning("/dummy/0.1.0/dummy/", web)?);

            env.fake_release()
                .name("dummy")
                .version("0.2.0")
                .archive_storage(archive_storage)
                .rustdoc_file("dummy/index.html")
                .yanked(true)
                .create()?;

            assert!(has_yanked_warning("/dummy/0.1.0/dummy/", web)?);

            Ok(())
        })
    }

    #[test]
    fn badges_are_urlencoded() {
        wrapper(|env| {
            use reqwest::Url;
            use url::Host;

            env.fake_release()
                .name("zstd")
                .version("0.5.1+zstd.1.4.4")
                .create()?;

            let frontend = env.override_frontend(|frontend| {
                use reqwest::blocking::Client;
                use reqwest::redirect::Policy;
                // avoid making network requests
                frontend.client = Client::builder().redirect(Policy::none()).build().unwrap();
            });
            let mut last_url = "/zstd/badge.svg".to_owned();
            let mut response = frontend.get(&last_url).send()?;
            let mut current_url = response.url().clone();
            // follow redirects until it actually goes out into the internet
            while !matches!(current_url.host(), Some(Host::Domain(_))) {
                println!("({} -> {})", last_url, current_url);
                assert_eq!(response.status(), StatusCode::MOVED_PERMANENTLY);
                last_url = response.url().to_string();
                response = frontend.get(response.url().as_str()).send().unwrap();
                current_url = Url::parse(response.headers()[reqwest::header::LOCATION].to_str()?)?;
            }
            println!("({} -> {})", last_url, current_url);
            assert_eq!(response.status(), StatusCode::MOVED_PERMANENTLY);
            assert_eq!(
                current_url.as_str(),
                "https://img.shields.io/docsrs/zstd/latest"
            );
            // make sure we aren't actually making network requests
            assert_ne!(
                response.url().as_str(),
                "https://img.shields.io/docsrs/zstd/latest"
            );

            Ok(())
        })
    }

    #[test_case(true)]
    #[test_case(false)]
    fn crate_name_percent_decoded_redirect(archive_storage: bool) {
        wrapper(|env| {
            env.fake_release()
                .name("fake-crate")
                .version("0.0.1")
                .archive_storage(archive_storage)
                .rustdoc_file("fake_crate/index.html")
                .create()?;

            let web = env.frontend();
            assert_redirect("/fake%2Dcrate", "/fake-crate/latest/fake_crate/", web)?;

            Ok(())
        });
    }

    #[test_case(true)]
    #[test_case(false)]
    fn base_redirect_handles_mismatched_separators(archive_storage: bool) {
        wrapper(|env| {
            let rels = [
                ("dummy-dash", "0.1.0"),
                ("dummy-dash", "0.2.0"),
                ("dummy_underscore", "0.1.0"),
                ("dummy_underscore", "0.2.0"),
                ("dummy_mixed-separators", "0.1.0"),
                ("dummy_mixed-separators", "0.2.0"),
            ];

            for (name, version) in &rels {
                env.fake_release()
                    .name(name)
                    .version(version)
                    .archive_storage(archive_storage)
                    .rustdoc_file(&(name.replace('-', "_") + "/index.html"))
                    .create()?;
            }

            let web = env.frontend();

            assert_redirect("/dummy_dash", "/dummy-dash/latest/dummy_dash/", web)?;
            assert_redirect("/dummy_dash/*", "/dummy-dash/0.2.0/dummy_dash/", web)?;
            assert_redirect("/dummy_dash/0.1.0", "/dummy-dash/0.1.0/dummy_dash/", web)?;
            assert_redirect(
                "/dummy-underscore",
                "/dummy_underscore/latest/dummy_underscore/",
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
                "/dummy_mixed-separators/latest/dummy_mixed_separators/",
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

    #[test_case(true)]
    #[test_case(false)]
    fn specific_pages_do_not_handle_mismatched_separators(archive_storage: bool) {
        wrapper(|env| {
            env.fake_release()
                .name("dummy-dash")
                .version("0.1.0")
                .archive_storage(archive_storage)
                .rustdoc_file("dummy_dash/index.html")
                .create()?;

            env.fake_release()
                .name("dummy_mixed-separators")
                .version("0.1.0")
                .archive_storage(archive_storage)
                .rustdoc_file("dummy_mixed_separators/index.html")
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

    #[test_case(true)]
    #[test_case(false)]
    fn platform_links_go_to_current_path(archive_storage: bool) {
        fn get_platform_links(
            path: &str,
            web: &TestFrontend,
        ) -> Result<Vec<(String, String, String)>, anyhow::Error> {
            assert_success(path, web)?;
            let data = web.get(path).send()?.text()?;
            let dom = kuchiki::parse_html().one(data);
            Ok(dom
                .select(r#"a[aria-label="Platform"] + ul li a"#)
                .expect("invalid selector")
                .map(|el| {
                    let attributes = el.attributes.borrow();
                    let url = attributes.get("href").expect("href").to_string();
                    let rel = attributes.get("rel").unwrap_or("").to_string();
                    let name = el.text_contents();
                    (name, url, rel)
                })
                .collect())
        }

        fn assert_platform_links(
            web: &TestFrontend,
            path: &str,
            links: &[(&str, &str)],
        ) -> Result<(), anyhow::Error> {
            let mut links: BTreeMap<_, _> = links.iter().copied().collect();

            for (platform, link, rel) in get_platform_links(path, web)? {
                assert_eq!(rel, "nofollow");
                assert_redirect(&link, links.remove(platform.as_str()).unwrap(), web)?;
            }

            assert!(links.is_empty());

            Ok(())
        }

        wrapper(|env| {
            let web = env.frontend();

            // no explicit default-target
            env.fake_release()
                .name("dummy")
                .version("0.1.0")
                .archive_storage(archive_storage)
                .rustdoc_file("dummy/index.html")
                .rustdoc_file("dummy/struct.Dummy.html")
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

            assert_platform_links(
                web,
                "/dummy/latest/dummy/",
                &[("x86_64-unknown-linux-gnu", "/dummy/latest/dummy/index.html")],
            )?;

            assert_platform_links(
                web,
                "/dummy/latest/dummy/index.html",
                &[("x86_64-unknown-linux-gnu", "/dummy/latest/dummy/index.html")],
            )?;

            assert_platform_links(
                web,
                "/dummy/latest/dummy/struct.Dummy.html",
                &[(
                    "x86_64-unknown-linux-gnu",
                    "/dummy/latest/dummy/struct.Dummy.html",
                )],
            )?;

            // set an explicit target that requires cross-compile
            env.fake_release()
                .name("dummy")
                .version("0.2.0")
                .archive_storage(archive_storage)
                .rustdoc_file("dummy/index.html")
                .rustdoc_file("dummy/struct.Dummy.html")
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

            assert_platform_links(
                web,
                "/dummy/latest/dummy/",
                &[("x86_64-pc-windows-msvc", "/dummy/latest/dummy/index.html")],
            )?;

            assert_platform_links(
                web,
                "/dummy/latest/dummy/index.html",
                &[("x86_64-pc-windows-msvc", "/dummy/latest/dummy/index.html")],
            )?;

            assert_platform_links(
                web,
                "/dummy/latest/dummy/struct.Dummy.html",
                &[(
                    "x86_64-pc-windows-msvc",
                    "/dummy/latest/dummy/struct.Dummy.html",
                )],
            )?;

            // set an explicit target without cross-compile
            env.fake_release()
                .name("dummy")
                .version("0.3.0")
                .archive_storage(archive_storage)
                .rustdoc_file("dummy/index.html")
                .rustdoc_file("dummy/struct.Dummy.html")
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

            assert_platform_links(
                web,
                "/dummy/latest/dummy/",
                &[("x86_64-unknown-linux-gnu", "/dummy/latest/dummy/index.html")],
            )?;

            assert_platform_links(
                web,
                "/dummy/latest/dummy/index.html",
                &[("x86_64-unknown-linux-gnu", "/dummy/latest/dummy/index.html")],
            )?;

            assert_platform_links(
                web,
                "/dummy/latest/dummy/struct.Dummy.html",
                &[(
                    "x86_64-unknown-linux-gnu",
                    "/dummy/latest/dummy/struct.Dummy.html",
                )],
            )?;

            // multiple targets
            env.fake_release()
                .name("dummy")
                .version("0.4.0")
                .archive_storage(archive_storage)
                .rustdoc_file("settings.html")
                .rustdoc_file("dummy/index.html")
                .rustdoc_file("dummy/struct.Dummy.html")
                .rustdoc_file("dummy/struct.DefaultOnly.html")
                .rustdoc_file("x86_64-pc-windows-msvc/settings.html")
                .rustdoc_file("x86_64-pc-windows-msvc/dummy/index.html")
                .rustdoc_file("x86_64-pc-windows-msvc/dummy/struct.Dummy.html")
                .rustdoc_file("x86_64-pc-windows-msvc/dummy/struct.WindowsOnly.html")
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
                "/dummy/latest/settings.html",
                &[
                    (
                        "x86_64-pc-windows-msvc",
                        "/dummy/latest/x86_64-pc-windows-msvc/settings.html",
                    ),
                    ("x86_64-unknown-linux-gnu", "/dummy/latest/settings.html"),
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

    #[test]
    fn test_target_redirect_not_found() {
        wrapper(|env| {
            let web = env.frontend();
            assert_eq!(
                web.get("/crate/fdsafdsafdsafdsa/0.1.0/target-redirect/x86_64-apple-darwin/")
                    .send()?
                    .status(),
                StatusCode::NOT_FOUND,
            );
            Ok(())
        })
    }

    #[test]
    fn test_redirect_to_latest_301() {
        wrapper(|env| {
            env.fake_release().name("dummy").version("1.0.0").create()?;
            let web = env.frontend();
            let client = ClientBuilder::new()
                .redirect(redirect::Policy::none())
                .build()
                .unwrap();
            let url = format!("http://{}/dummy", web.server_addr());
            let resp = client.get(url).send()?;
            assert_eq!(resp.status(), StatusCode::MOVED_PERMANENTLY);
            assert_eq!(
                resp.headers().get("Cache-Control").unwrap(),
                reqwest::header::HeaderValue::from_str("max-age=86400").unwrap()
            );
            assert!(resp
                .headers()
                .get("Location")
                .unwrap()
                .to_str()
                .unwrap()
                .contains("/dummy/latest/dummy/"));
            Ok(())
        })
    }

    #[test_case(true)]
    #[test_case(false)]
    fn test_fully_yanked_crate_404s(archive_storage: bool) {
        wrapper(|env| {
            env.fake_release()
                .name("dummy")
                .version("1.0.0")
                .archive_storage(archive_storage)
                .yanked(true)
                .create()?;

            assert_eq!(
                env.frontend().get("/crate/dummy").send()?.status(),
                StatusCode::NOT_FOUND
            );

            assert_eq!(
                env.frontend().get("/dummy").send()?.status(),
                StatusCode::NOT_FOUND
            );

            Ok(())
        })
    }

    #[test_case(true)]
    #[test_case(false)]
    fn test_no_trailing_target_slash(archive_storage: bool) {
        // regression test for https://github.com/rust-lang/docs.rs/issues/856
        wrapper(|env| {
            env.fake_release()
                .name("dummy")
                .version("0.1.0")
                .archive_storage(archive_storage)
                .create()?;
            let web = env.frontend();
            assert_redirect(
                "/crate/dummy/0.1.0/target-redirect/x86_64-apple-darwin",
                "/dummy/0.1.0/dummy/",
                web,
            )?;
            env.fake_release()
                .name("dummy")
                .version("0.2.0")
                .archive_storage(archive_storage)
                .add_platform("x86_64-apple-darwin")
                .create()?;
            assert_redirect(
                "/crate/dummy/0.2.0/target-redirect/x86_64-apple-darwin",
                "/dummy/0.2.0/x86_64-apple-darwin/dummy/",
                web,
            )?;
            assert_redirect(
                "/crate/dummy/0.2.0/target-redirect/platform-that-does-not-exist",
                "/dummy/0.2.0/dummy/",
                web,
            )?;
            Ok(())
        })
    }

    #[test]
    fn test_redirect_crate_coloncolon_path() {
        wrapper(|env| {
            let web = env.frontend();
            env.fake_release().name("some_random_crate").create()?;
            env.fake_release().name("some_other_crate").create()?;

            assert_redirect(
                "/some_random_crate::somepath",
                "/some_random_crate/latest/some_random_crate/?search=somepath",
                web,
            )?;
            assert_redirect(
                "/some_random_crate::some::path",
                "/some_random_crate/latest/some_random_crate/?search=some::path",
                web,
            )?;
            Ok(())
        })
    }

    #[test]
    // regression test for https://github.com/rust-lang/docs.rs/pull/885#issuecomment-655147643
    fn test_no_panic_on_missing_kind() {
        wrapper(|env| {
            let db = env.db();
            let id = env
                .fake_release()
                .name("strum")
                .version("0.13.0")
                .create()?;
            // https://stackoverflow.com/questions/18209625/how-do-i-modify-fields-inside-the-new-postgresql-json-datatype
            db.conn().query(
                r#"UPDATE releases SET dependencies = dependencies::jsonb #- '{0,2}' WHERE id = $1"#,
                &[&id],
            )?;
            let web = env.frontend();
            assert_success("/strum/0.13.0/strum/", web)?;
            assert_success("/crate/strum/0.13.0/", web)?;
            Ok(())
        })
    }

    #[test]
    // regression test for https://github.com/rust-lang/docs.rs/pull/885#issuecomment-655154405
    fn test_readme_rendered_as_html() {
        wrapper(|env| {
            let readme = "# Overview";
            env.fake_release()
                .name("strum")
                .version("0.18.0")
                .readme(readme)
                .create()?;
            let page = kuchiki::parse_html()
                .one(env.frontend().get("/crate/strum/0.18.0").send()?.text()?);
            let rendered = page.select_first("#main").expect("missing readme");
            println!("{}", rendered.text_contents());
            rendered
                .as_node()
                .select_first("h1")
                .expect("`# Overview` was not rendered as HTML");
            Ok(())
        })
    }

    #[test]
    // regression test for https://github.com/rust-lang/docs.rs/pull/885#issuecomment-655149288
    fn test_build_status_is_accurate() {
        wrapper(|env| {
            env.fake_release()
                .name("hexponent")
                .version("0.3.0")
                .create()?;
            env.fake_release()
                .name("hexponent")
                .version("0.2.0")
                .build_result_failed()
                .create()?;
            let web = env.frontend();

            let status = |version| -> Result<_, anyhow::Error> {
                let page =
                    kuchiki::parse_html().one(web.get("/crate/hexponent/0.3.0").send()?.text()?);
                let selector = format!(r#"ul > li a[href="/crate/hexponent/{}"]"#, version);
                let anchor = page
                    .select(&selector)
                    .unwrap()
                    .find(|a| a.text_contents().trim() == version)
                    .unwrap();
                let attributes = anchor.as_node().as_element().unwrap().attributes.borrow();
                let classes = attributes.get("class").unwrap();
                Ok(classes.split(' ').all(|c| c != "warn"))
            };

            assert!(status("0.3.0")?);
            assert!(!status("0.2.0")?);
            Ok(())
        })
    }

    #[test_case(true)]
    #[test_case(false)]
    fn test_no_trailing_rustdoc_slash(archive_storage: bool) {
        wrapper(|env| {
            env.fake_release()
                .name("tokio")
                .version("0.2.21")
                .archive_storage(archive_storage)
                .rustdoc_file("tokio/time/index.html")
                .create()?;
            assert_redirect(
                "/tokio/0.2.21/tokio/time",
                "/tokio/0.2.21/tokio/time/index.html",
                env.frontend(),
            )
        })
    }

    #[test_case(true)]
    #[test_case(false)]
    fn test_non_ascii(archive_storage: bool) {
        wrapper(|env| {
            env.fake_release()
                .name("const_unit_poc")
                .version("1.0.0")
                .archive_storage(archive_storage)
                .rustdoc_file("const_unit_poc/units/constant.Ω.html")
                .create()?;
            assert_success(
                "/const_unit_poc/1.0.0/const_unit_poc/units/constant.Ω.html",
                env.frontend(),
            )
        })
    }

    #[test_case(true)]
    #[test_case(false)]
    fn test_latest_version_keeps_query(archive_storage: bool) {
        wrapper(|env| {
            env.fake_release()
                .name("tungstenite")
                .version("0.10.0")
                .archive_storage(archive_storage)
                .rustdoc_file("tungstenite/index.html")
                .create()?;
            env.fake_release()
                .name("tungstenite")
                .version("0.11.0")
                .archive_storage(archive_storage)
                .rustdoc_file("tungstenite/index.html")
                .create()?;
            assert_eq!(
                latest_version_redirect(
                    "/tungstenite/0.10.0/tungstenite/?search=String%20-%3E%20Message",
                    env.frontend()
                )?,
                "/crate/tungstenite/latest/target-redirect/x86_64-unknown-linux-gnu/tungstenite/index.html?search=String%20-%3E%20Message",
            );
            Ok(())
        });
    }

    #[test_case(true)]
    #[test_case(false)]
    fn latest_version_works_when_source_deleted(archive_storage: bool) {
        wrapper(|env| {
            env.fake_release()
                .name("pyo3")
                .version("0.2.7")
                .archive_storage(archive_storage)
                .source_file("src/objects/exc.rs", b"//! some docs")
                .create()?;
            env.fake_release().name("pyo3").version("0.13.2").create()?;
            let target_redirect = "/crate/pyo3/latest/target-redirect/x86_64-unknown-linux-gnu/src/pyo3/objects/exc.rs.html";
            assert_eq!(
                latest_version_redirect(
                    "/pyo3/0.2.7/src/pyo3/objects/exc.rs.html",
                    env.frontend()
                )?,
                target_redirect
            );
            assert_redirect(
                target_redirect,
                "/pyo3/latest/pyo3/?search=exc",
                env.frontend(),
            )?;
            Ok(())
        })
    }

    #[test_case(true)]
    #[test_case(false)]
    fn test_version_link_goes_to_docs(archive_storage: bool) {
        wrapper(|env| {
            env.fake_release()
                .name("hexponent")
                .version("0.3.0")
                .archive_storage(archive_storage)
                .rustdoc_file("hexponent/index.html")
                .create()?;
            env.fake_release()
                .name("hexponent")
                .version("0.3.1")
                .archive_storage(archive_storage)
                .rustdoc_file("hexponent/index.html")
                .create()?;

            // test rustdoc pages stay on the documentation
            let page = kuchiki::parse_html().one(
                env.frontend()
                    .get("/hexponent/0.3.0/hexponent/")
                    .send()?
                    .text()?,
            );
            let selector =
                r#"ul > li a[href="/crate/hexponent/0.3.1/target-redirect/hexponent/index.html"]"#
                    .to_string();
            assert_eq!(
                page.select(&selector).unwrap().count(),
                1,
                "link to /target-redirect/ not found"
            );

            // test /crate pages stay on /crate
            let page = kuchiki::parse_html().one(
                env.frontend()
                    .get("/crate/hexponent/0.3.0/")
                    .send()?
                    .text()?,
            );
            let selector = r#"ul > li a[href="/crate/hexponent/0.3.1"]"#.to_string();
            assert_eq!(
                page.select(&selector).unwrap().count(),
                1,
                "link to /crate not found"
            );

            Ok(())
        })
    }

    #[test]
    fn test_repository_link_in_topbar_dropdown() {
        wrapper(|env| {
            env.fake_release()
                .name("testing")
                .repo("https://git.example.com")
                .version("0.1.0")
                .rustdoc_file("testing/index.html")
                .create()?;

            let dom = kuchiki::parse_html().one(
                env.frontend()
                    .get("/testing/0.1.0/testing/")
                    .send()?
                    .text()?,
            );

            assert_eq!(
                dom.select(r#"ul > li a[href="https://git.example.com"]"#)
                    .unwrap()
                    .count(),
                1,
            );

            Ok(())
        })
    }

    #[test]
    fn test_repository_link_in_topbar_dropdown_github() {
        wrapper(|env| {
            env.fake_release()
                .name("testing")
                .version("0.1.0")
                .rustdoc_file("testing/index.html")
                .github_stats("https://git.example.com", 123, 321, 333)
                .create()?;

            let dom = kuchiki::parse_html().one(
                env.frontend()
                    .get("/testing/0.1.0/testing/")
                    .send()?
                    .text()?,
            );

            assert_eq!(
                dom.select(r#"ul > li a[href="https://git.example.com"]"#)
                    .unwrap()
                    .count(),
                1,
            );

            Ok(())
        })
    }

    #[test_case(true)]
    #[test_case(false)]
    fn test_missing_target_redirects_to_search(archive_storage: bool) {
        wrapper(|env| {
            env.fake_release()
                .name("winapi")
                .version("0.3.9")
                .archive_storage(archive_storage)
                .rustdoc_file("winapi/macro.ENUM.html")
                .create()?;

            assert_redirect(
                "/winapi/0.3.9/x86_64-unknown-linux-gnu/winapi/macro.ENUM.html",
                "/winapi/0.3.9/winapi/macro.ENUM.html",
                env.frontend(),
            )?;
            assert_not_found("/winapi/0.3.9/winapi/struct.not_here.html", env.frontend())?;

            Ok(())
        })
    }

    #[test_case(true)]
    #[test_case(false)]
    fn test_redirect_source_not_rust(archive_storage: bool) {
        wrapper(|env| {
            env.fake_release()
                .name("winapi")
                .version("0.3.8")
                .archive_storage(archive_storage)
                .source_file("src/docs.md", b"created by Peter Rabbit")
                .create()?;

            env.fake_release()
                .name("winapi")
                .version("0.3.9")
                .archive_storage(archive_storage)
                .create()?;

            assert_success("/winapi/0.3.8/src/winapi/docs.md.html", env.frontend())?;
            // people can end up here from clicking "go to latest" while in source view
            assert_redirect(
                "/crate/winapi/0.3.9/target-redirect/src/winapi/docs.md.html",
                "/winapi/0.3.9/winapi/",
                env.frontend(),
            )?;
            Ok(())
        })
    }

    #[test]
    fn canonical_url() {
        wrapper(|env| {
            env.fake_release()
                .name("dummy-dash")
                .version("0.1.0")
                .documentation_url(Some("http://example.com".to_string()))
                .rustdoc_file("dummy_dash/index.html")
                .create()?;

            env.fake_release()
                .name("dummy-docs")
                .version("0.1.0")
                .documentation_url(Some("https://docs.rs/foo".to_string()))
                .rustdoc_file("dummy_docs/index.html")
                .create()?;

            env.fake_release()
                .name("dummy-nodocs")
                .version("0.1.0")
                .documentation_url(None)
                .rustdoc_file("dummy_nodocs/index.html")
                .rustdoc_file("dummy_nodocs/struct.Foo.html")
                .create()?;

            let web = env.frontend();

            assert!(web
                .get("/dummy-dash/0.1.0/dummy_dash/")
                .send()?
                .text()?
                .contains("rel=\"canonical\""),);

            assert!(web
                .get("/dummy-docs/0.1.0/dummy_docs/")
                .send()?
                .text()?
                .contains(
                "<link rel=\"canonical\" href=\"https://docs.rs/dummy-docs/latest/dummy_docs/\" />"
            ),);

            assert!(
                web
                    .get("/dummy-nodocs/0.1.0/dummy_nodocs/")
                    .send()?
                    .text()?
                    .contains("<link rel=\"canonical\" href=\"https://docs.rs/dummy-nodocs/latest/dummy_nodocs/\" />"),
            );

            assert!(
                web
                    .get("/dummy-nodocs/0.1.0/dummy_nodocs/struct.Foo.html")
                    .send()?
                    .text()?
                    .contains("<link rel=\"canonical\" href=\"https://docs.rs/dummy-nodocs/latest/dummy_nodocs/struct.Foo.html\" />"),
            );
            Ok(())
        })
    }
}
