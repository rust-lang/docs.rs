//! Web interface of docs.rs

pub mod page;

use crate::utils::get_correct_docsrs_style_file;
use crate::utils::{report_error, spawn_blocking};
use anyhow::{anyhow, bail, Context as _, Result};
use axum_extra::middleware::option_layer;
use serde_json::Value;
use tracing::{info, instrument};

mod build_details;
mod builds;
pub(crate) mod cache;
pub(crate) mod crate_details;
mod csp;
pub(crate) mod error;
mod features;
mod file;
mod headers;
mod highlight;
mod markdown;
pub(crate) mod metrics;
mod releases;
mod routes;
mod rustdoc;
mod sitemap;
mod source;
mod statics;
mod status;

use crate::{db::Pool, impl_axum_webpage, Context};
use anyhow::Error;
use axum::{
    extract::Extension,
    http::Request as AxumRequest,
    http::StatusCode,
    middleware,
    middleware::Next,
    response::{IntoResponse, Response as AxumResponse},
    Router as AxumRouter,
};
use chrono::{DateTime, Utc};
use error::AxumNope;
use page::TemplateData;
use percent_encoding::{utf8_percent_encode, AsciiSet, CONTROLS};
use postgres::Client;
use semver::{Version, VersionReq};
use serde::Serialize;
use std::net::{IpAddr, Ipv4Addr};
use std::{
    borrow::{Borrow, Cow},
    net::SocketAddr,
    sync::Arc,
};
use tower::ServiceBuilder;
use tower_http::{catch_panic::CatchPanicLayer, timeout::TimeoutLayer, trace::TraceLayer};
use url::form_urlencoded;

// from https://github.com/servo/rust-url/blob/master/url/src/parser.rs
// and https://github.com/tokio-rs/axum/blob/main/axum-extra/src/lib.rs
const FRAGMENT: &AsciiSet = &CONTROLS.add(b' ').add(b'"').add(b'<').add(b'>').add(b'`');
const PATH: &AsciiSet = &FRAGMENT.add(b'#').add(b'?').add(b'{').add(b'}');

pub(crate) fn encode_url_path(path: &str) -> String {
    utf8_percent_encode(path, PATH).to_string()
}

const DEFAULT_BIND: SocketAddr = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(0, 0, 0, 0)), 3000);

#[derive(Debug)]
struct MatchVersion {
    /// Represents the crate name that was found when attempting to load a crate release.
    ///
    /// `match_version` will attempt to match a provided crate name against similar crate names with
    /// dashes (`-`) replaced with underscores (`_`) and vice versa.
    pub corrected_name: Option<String>,
    pub version: MatchSemver,
    pub rustdoc_status: bool,
    pub target_name: String,
}

impl MatchVersion {
    /// If the matched version was an exact match to the requested crate name, returns the
    /// `MatchSemver` for the query. If the lookup required a dash/underscore conversion, returns
    /// `CrateNotFound`.
    fn exact_name_only(self) -> Result<MatchSemver, AxumNope> {
        if self.corrected_name.is_none() {
            Ok(self.version)
        } else {
            Err(AxumNope::CrateNotFound)
        }
    }
}

/// Represents the possible results of attempting to load a version requirement.
/// The id (i32) of the release is stored to simplify successive queries.
#[derive(Debug, Clone, PartialEq, Eq)]
enum MatchSemver {
    /// `match_version` was given an exact version, which matched a saved crate version.
    Exact((String, i32)),
    /// `match_version` was given a semver version requirement, which matched the given saved crate
    /// version.
    Semver((String, i32)),
    // `match_version` was given the string "latest", which matches the given saved crate version.
    Latest((String, i32)),
}

impl MatchSemver {
    /// Discard information about whether the loaded version was an exact match, and return the
    /// matched version string and id.
    pub fn into_parts(self) -> (String, i32) {
        match self {
            MatchSemver::Exact((v, i))
            | MatchSemver::Semver((v, i))
            | MatchSemver::Latest((v, i)) => (v, i),
        }
    }
}

/// Checks the database for crate releases that match the given name and version.
///
/// `version` may be an exact version number or loose semver version requirement. The return value
/// will indicate whether the given version exactly matched a version number from the database.
///
/// This function will also check for crates where dashes in the name (`-`) have been replaced with
/// underscores (`_`) and vice-versa. The return value will indicate whether the crate name has
/// been matched exactly, or if there has been a "correction" in the name that matched instead.
fn match_version(
    conn: &mut Client,
    name: &str,
    input_version: Option<&str>,
) -> Result<MatchVersion, AxumNope> {
    let (crate_id, corrected_name) = {
        let rows = conn
            .query(
                "SELECT id, name
                 FROM crates
                 WHERE normalize_crate_name(name) = normalize_crate_name($1)",
                &[&name],
            )
            .context("error fetching crate")?;

        let row = rows.get(0).ok_or(AxumNope::CrateNotFound)?;

        let id: i32 = row.get(0);
        let db_name = row.get(1);
        if db_name != name {
            (id, Some(db_name))
        } else {
            (id, None)
        }
    };

    // first load and parse all versions of this crate,
    // skipping and reporting versions that are not semver valid.
    // `releases_for_crate` is already sorted, newest version first.
    let releases = crate_details::releases_for_crate(conn, crate_id)
        .expect("error fetching releases for crate");

    if releases.is_empty() {
        return Err(AxumNope::CrateNotFound);
    }

    // version is an Option<&str> from router::Router::get, need to decode first.
    // Any encoding errors we treat as _any version_.
    let req_version = input_version.unwrap_or("*");

    // first check for exact match, we can't expect users to use semver in query
    if let Ok(parsed_req_version) = Version::parse(req_version) {
        if let Some(release) = releases
            .iter()
            .find(|release| release.version == parsed_req_version)
        {
            return Ok(MatchVersion {
                corrected_name,
                version: MatchSemver::Exact((release.version.to_string(), release.id)),
                rustdoc_status: release.rustdoc_status,
                target_name: release.target_name.clone(),
            });
        }
    }

    // Now try to match with semver, treat `newest` and `latest` as `*`
    let req_semver = if req_version == "newest" || req_version == "latest" {
        VersionReq::STAR
    } else {
        VersionReq::parse(req_version).map_err(|err| {
            info!(
                "could not parse version requirement \"{}\": {:?}",
                req_version, err
            );
            AxumNope::VersionNotFound
        })?
    };

    // starting here, we only look at non-yanked releases
    let releases: Vec<_> = releases.iter().filter(|r| !r.yanked).collect();

    // try to match the version in all un-yanked releases.
    if let Some(release) = releases
        .iter()
        .find(|release| req_semver.matches(&release.version))
    {
        return Ok(MatchVersion {
            corrected_name,
            version: if input_version == Some("latest") {
                MatchSemver::Latest((release.version.to_string(), release.id))
            } else {
                MatchSemver::Semver((release.version.to_string(), release.id))
            },
            rustdoc_status: release.rustdoc_status,
            target_name: release.target_name.clone(),
        });
    }

    // semver `*` does not match pre-releases.
    // When someone wants the latest release and we have only pre-releases
    // just return the latest prerelease.
    if req_semver == VersionReq::STAR {
        return releases
            .first()
            .map(|release| MatchVersion {
                corrected_name: corrected_name.clone(),
                version: MatchSemver::Semver((release.version.to_string(), release.id)),
                rustdoc_status: release.rustdoc_status,
                target_name: release.target_name.clone(),
            })
            .ok_or(AxumNope::VersionNotFound);
    }

    // Since we return with a CrateNotFound earlier if the db reply is empty,
    // we know that versions were returned but none satisfied the version requirement.
    // This can only happen when all versions are yanked.
    Err(AxumNope::VersionNotFound)
}

// temporary wrapper around `match_version` for axum handlers.
//
// FIXME: this can go when we fully migrated to axum / async in web
async fn match_version_axum(
    pool: &Pool,
    name: &str,
    input_version: Option<&str>,
) -> Result<MatchVersion, Error> {
    spawn_blocking({
        let name = name.to_owned();
        let input_version = input_version.map(str::to_owned);
        let pool = pool.clone();
        move || {
            let mut conn = pool.get()?;
            Ok(match_version(&mut conn, &name, input_version.as_deref())?)
        }
    })
    .await
}

async fn log_timeouts_to_sentry<B>(req: AxumRequest<B>, next: Next<B>) -> AxumResponse {
    let uri = req.uri().clone();

    let response = next.run(req).await;

    if response.status() == StatusCode::REQUEST_TIMEOUT {
        tracing::error!(?uri, "request timeout");
    }

    response
}

fn apply_middleware(
    router: AxumRouter,
    context: &dyn Context,
    template_data: Option<Arc<TemplateData>>,
) -> Result<AxumRouter> {
    let config = context.config()?;
    let has_templates = template_data.is_some();
    Ok(router.layer(
        ServiceBuilder::new()
            .layer(TraceLayer::new_for_http())
            .layer(sentry_tower::NewSentryLayer::new_from_top())
            .layer(sentry_tower::SentryHttpLayer::with_transaction())
            .layer(CatchPanicLayer::new())
            .layer(option_layer(
                config
                    .report_request_timeouts
                    .then_some(middleware::from_fn(log_timeouts_to_sentry)),
            ))
            .layer(option_layer(config.request_timeout.map(TimeoutLayer::new)))
            .layer(Extension(context.pool()?))
            .layer(Extension(context.build_queue()?))
            .layer(Extension(context.service_metrics()?))
            .layer(Extension(context.instance_metrics()?))
            .layer(Extension(context.config()?))
            .layer(Extension(context.storage()?))
            .layer(Extension(context.repository_stats_updater()?))
            .layer(option_layer(template_data.map(Extension)))
            .layer(middleware::from_fn(csp::csp_middleware))
            .layer(option_layer(has_templates.then_some(middleware::from_fn(
                page::web_page::render_templates_middleware,
            ))))
            .layer(middleware::from_fn(cache::cache_middleware)),
    ))
}

pub(crate) fn build_axum_app(
    context: &dyn Context,
    template_data: Arc<TemplateData>,
) -> Result<AxumRouter, Error> {
    apply_middleware(routes::build_axum_routes(), context, Some(template_data))
}

pub(crate) fn build_metrics_axum_app(context: &dyn Context) -> Result<AxumRouter, Error> {
    apply_middleware(routes::build_metric_routes(), context, None)
}

pub fn start_background_metrics_webserver(
    addr: Option<SocketAddr>,
    context: &dyn Context,
) -> Result<(), Error> {
    let axum_addr: SocketAddr = addr.unwrap_or(DEFAULT_BIND);

    tracing::info!(
        "Starting metrics web server on `{}:{}`",
        axum_addr.ip(),
        axum_addr.port()
    );

    let metrics_axum_app = build_metrics_axum_app(context)?.into_make_service();
    let runtime = context.runtime()?;

    runtime.spawn(async move {
        if let Err(err) = axum::Server::bind(&axum_addr)
            .serve(metrics_axum_app)
            .await
            .context("error running metrics web server")
        {
            report_error(&err);
        }
    });

    Ok(())
}

#[instrument(skip_all)]
pub fn start_web_server(addr: Option<SocketAddr>, context: &dyn Context) -> Result<(), Error> {
    let template_data = Arc::new(TemplateData::new(
        &mut *context.pool()?.get()?,
        context.config()?.render_threads,
    )?);

    let axum_addr = addr.unwrap_or(DEFAULT_BIND);

    tracing::info!(
        "Starting web server on `{}:{}`",
        axum_addr.ip(),
        axum_addr.port()
    );

    // initialize the storage and the repo-updater in sync context
    // so it can stay sync for now and doesn't fail when they would
    // be initialized while starting the server below.
    context.storage()?;
    context.repository_stats_updater()?;

    context.runtime()?.block_on(async {
        axum::Server::bind(&axum_addr)
            .serve(build_axum_app(context, template_data)?.into_make_service())
            .with_graceful_shutdown(shutdown_signal())
            .await?;
        Ok::<(), Error>(())
    })?;

    Ok(())
}

async fn shutdown_signal() {
    let ctrl_c = async {
        tokio::signal::ctrl_c()
            .await
            .expect("failed to install Ctrl+C handler");
    };

    #[cfg(unix)]
    let terminate = async {
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("failed to install signal handler")
            .recv()
            .await;
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {},
        _ = terminate => {},
    }

    info!("signal received, starting graceful shutdown");
}

/// Converts Timespec to nice readable relative time string
fn duration_to_str(init: DateTime<Utc>) -> String {
    let now = Utc::now();
    let delta = now.signed_duration_since(init);

    let delta = (
        delta.num_days(),
        delta.num_hours(),
        delta.num_minutes(),
        delta.num_seconds(),
    );

    match delta {
        (days, ..) if days > 5 => format!("{}", init.format("%b %d, %Y")),
        (days @ 2..=5, ..) => format!("{days} days ago"),
        (1, ..) => "one day ago".to_string(),

        (_, hours, ..) if hours > 1 => format!("{hours} hours ago"),
        (_, 1, ..) => "an hour ago".to_string(),

        (_, _, minutes, _) if minutes > 1 => format!("{minutes} minutes ago"),
        (_, _, 1, _) => "one minute ago".to_string(),

        (_, _, _, seconds) if seconds > 0 => format!("{seconds} seconds ago"),
        _ => "just now".to_string(),
    }
}

#[instrument]
fn axum_redirect<U>(uri: U) -> Result<impl IntoResponse, Error>
where
    U: TryInto<http::Uri> + std::fmt::Debug,
    <U as TryInto<http::Uri>>::Error: std::fmt::Debug,
{
    let uri: http::Uri = uri
        .try_into()
        .map_err(|err| anyhow!("invalid URI: {:?}", err))?;

    if let Some(path_and_query) = uri.path_and_query() {
        if path_and_query.as_str().starts_with("//") {
            bail!("protocol relative redirects are forbidden");
        }
    } else {
        // we always want a path to redirect to, even when it's just `/`
        bail!("missing path in URI");
    }

    Ok((
        StatusCode::FOUND,
        [(
            http::header::LOCATION,
            http::HeaderValue::try_from(uri.to_string()).context("invalid uri for redirect")?,
        )],
    ))
}

#[instrument]
fn axum_cached_redirect<U>(
    uri: U,
    cache_policy: cache::CachePolicy,
) -> Result<impl IntoResponse, Error>
where
    U: TryInto<http::Uri> + std::fmt::Debug,
    <U as TryInto<http::Uri>>::Error: std::fmt::Debug,
{
    let mut resp = axum_redirect(uri)?.into_response();
    resp.extensions_mut().insert(cache_policy);
    Ok(resp)
}

/// Parse an URI into a http::Uri struct.
/// When `queries` are given these are added to the URL,
/// with empty `queries` the `?` will be omitted.
pub(crate) fn axum_parse_uri_with_params<I, K, V>(uri: &str, queries: I) -> Result<http::Uri, Error>
where
    I: IntoIterator,
    I::Item: Borrow<(K, V)>,
    K: AsRef<str>,
    V: AsRef<str>,
{
    let mut queries = queries.into_iter().peekable();
    if queries.peek().is_some() {
        let query_params: String = form_urlencoded::Serializer::new(String::new())
            .extend_pairs(queries)
            .finish();
        format!("{uri}?{query_params}")
            .parse::<http::Uri>()
            .context("error parsing URL")
    } else {
        uri.parse::<http::Uri>().context("error parsing URL")
    }
}

/// MetaData used in header
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct MetaData {
    pub(crate) name: String,
    // If we're on a page with /latest/ in the URL, the string "latest".
    // Otherwise, the version as a string.
    pub(crate) version_or_latest: String,
    // The exact version of the crate being shown. Never contains "latest".
    pub(crate) version: String,
    pub(crate) description: Option<String>,
    pub(crate) target_name: Option<String>,
    pub(crate) rustdoc_status: bool,
    pub(crate) default_target: String,
    pub(crate) doc_targets: Vec<String>,
    pub(crate) yanked: bool,
    /// CSS file to use depending on the rustdoc version used to generate this version of this
    /// crate.
    pub(crate) rustdoc_css_file: String,
}

impl MetaData {
    fn from_crate(
        conn: &mut Client,
        name: &str,
        version: &str,
        version_or_latest: &str,
    ) -> Result<MetaData> {
        conn.query_opt(
            "SELECT crates.name,
                       releases.version,
                       releases.description,
                       releases.target_name,
                       releases.rustdoc_status,
                       releases.default_target,
                       releases.doc_targets,
                       releases.yanked,
                       releases.doc_rustc_version
                FROM releases
                INNER JOIN crates ON crates.id = releases.crate_id
                WHERE crates.name = $1 AND releases.version = $2",
            &[&name, &version],
        )?
        .map(|row| MetaData {
            name: row.get(0),
            version: row.get(1),
            version_or_latest: version_or_latest.to_string(),
            description: row.get(2),
            target_name: row.get(3),
            rustdoc_status: row.get(4),
            default_target: row.get(5),
            doc_targets: MetaData::parse_doc_targets(row.get(6)),
            yanked: row.get(7),
            rustdoc_css_file: get_correct_docsrs_style_file(row.get(8)).unwrap(),
        })
        .ok_or_else(|| anyhow!("missing metadata for {} {}", name, version))
    }

    fn parse_doc_targets(targets: Value) -> Vec<String> {
        targets
            .as_array()
            .map(|array| {
                array
                    .iter()
                    .filter_map(|item| item.as_str().map(|s| s.to_owned()))
                    .collect()
            })
            .unwrap_or_else(Vec::new)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub(crate) struct AxumErrorPage {
    /// The title of the page
    pub title: &'static str,
    /// The error message, displayed as a description
    pub message: Cow<'static, str>,
    #[serde(skip)]
    pub status: StatusCode,
}

impl_axum_webpage! {
    AxumErrorPage = "error.html",
    status = |err| err.status,
}

#[cfg(test)]
mod test {
    use super::*;
    use crate::{docbuilder::DocCoverage, test::*, web::match_version};
    use axum::http::StatusCode;
    use kuchiki::traits::TendrilSink;
    use serde_json::json;
    use test_case::test_case;

    fn release(version: &str, env: &TestEnvironment) -> i32 {
        env.fake_release()
            .name("foo")
            .version(version)
            .create()
            .unwrap()
    }

    fn version(v: Option<&str>, db: &TestDatabase) -> Option<String> {
        let version = match_version(&mut db.conn(), "foo", v)
            .ok()?
            .exact_name_only()
            .ok()?
            .into_parts()
            .0;
        Some(version)
    }

    #[allow(clippy::unnecessary_wraps)]
    fn semver(version: &'static str) -> Option<String> {
        Some(version.into())
    }

    #[allow(clippy::unnecessary_wraps)]
    fn exact(version: &'static str) -> Option<String> {
        Some(version.into())
    }

    fn clipboard_is_present_for_path(path: &str, web: &TestFrontend) -> bool {
        let data = web.get(path).send().unwrap().text().unwrap();
        let node = kuchiki::parse_html().one(data);
        node.select("#clipboard").unwrap().count() == 1
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
    fn test_doc_coverage_for_crate_pages() {
        wrapper(|env| {
            env.fake_release()
                .name("foo")
                .version("0.0.1")
                .source_file("test.rs", &[])
                .doc_coverage(DocCoverage {
                    total_items: 10,
                    documented_items: 6,
                    total_items_needing_examples: 2,
                    items_with_examples: 1,
                })
                .create()?;
            let web = env.frontend();

            let foo_crate = kuchiki::parse_html().one(web.get("/crate/foo/0.0.1").send()?.text()?);
            for value in &["60%", "6", "10", "2", "1"] {
                assert!(foo_crate
                    .select(".pure-menu-item b")
                    .unwrap()
                    .any(|e| dbg!(e.text_contents()).contains(value)));
            }

            let foo_doc = kuchiki::parse_html().one(web.get("/foo/0.0.1/foo").send()?.text()?);
            assert!(foo_doc
                .select(".pure-menu-link b")
                .unwrap()
                .any(|e| e.text_contents().contains("60%")));

            Ok(())
        });
    }

    #[test]
    fn test_show_clipboard_for_crate_pages() {
        wrapper(|env| {
            env.fake_release()
                .name("fake_crate")
                .version("0.0.1")
                .source_file("test.rs", &[])
                .create()
                .unwrap();
            let web = env.frontend();
            assert!(clipboard_is_present_for_path(
                "/crate/fake_crate/0.0.1",
                web
            ));
            assert!(clipboard_is_present_for_path(
                "/crate/fake_crate/0.0.1/source/",
                web
            ));
            assert!(clipboard_is_present_for_path(
                "/fake_crate/0.0.1/fake_crate",
                web
            ));
            Ok(())
        });
    }

    #[test]
    fn test_hide_clipboard_for_non_crate_pages() {
        wrapper(|env| {
            env.fake_release()
                .name("fake_crate")
                .version("0.0.1")
                .create()
                .unwrap();
            let web = env.frontend();
            assert!(!clipboard_is_present_for_path("/about", web));
            assert!(!clipboard_is_present_for_path("/releases", web));
            assert!(!clipboard_is_present_for_path("/", web));
            assert!(!clipboard_is_present_for_path("/not/a/real/path", web));
            Ok(())
        });
    }

    #[test]
    fn standard_library_redirects() {
        wrapper(|env| {
            let web = env.frontend();
            for krate in &["std", "alloc", "core", "proc_macro", "test"] {
                let target = format!("https://doc.rust-lang.org/stable/{krate}/");

                // with or without slash
                assert_redirect(&format!("/{krate}"), &target, web)?;
                assert_redirect(&format!("/{krate}/"), &target, web)?;
            }

            let target = "https://doc.rust-lang.org/stable/proc_macro/";
            // with or without slash
            assert_redirect("/proc-macro", target, web)?;
            assert_redirect("/proc-macro/", target, web)?;

            let target = "https://doc.rust-lang.org/nightly/nightly-rustc/";
            // with or without slash
            assert_redirect("/rustc", target, web)?;
            assert_redirect("/rustc/", target, web)?;

            let target = "https://doc.rust-lang.org/nightly/nightly-rustc/rustdoc/";
            // with or without slash
            assert_redirect("/rustdoc", target, web)?;
            assert_redirect("/rustdoc/", target, web)?;

            // queries are supported
            assert_redirect(
                "/std?search=foobar",
                "https://doc.rust-lang.org/stable/std/?search=foobar",
                web,
            )?;

            Ok(())
        })
    }

    #[test]
    fn double_slash_does_redirect_and_remove_slash() {
        wrapper(|env| {
            env.fake_release()
                .name("bat")
                .version("0.2.0")
                .create()
                .unwrap();
            let web = env.frontend();
            let response = web.get("/bat//").send()?;
            assert_eq!(response.status(), StatusCode::NOT_FOUND);
            Ok(())
        })
    }

    #[test]
    fn binary_docs_redirect_to_crate() {
        wrapper(|env| {
            env.fake_release()
                .name("bat")
                .version("0.2.0")
                .binary(true)
                .create()
                .unwrap();
            let web = env.frontend();
            assert_redirect("/bat/0.2.0", "/crate/bat/0.2.0", web)?;
            assert_redirect("/bat/0.2.0/i686-unknown-linux-gnu", "/crate/bat/0.2.0", web)?;
            /* TODO: this should work (https://github.com/rust-lang/docs.rs/issues/603)
            assert_redirect("/bat/0.2.0/i686-unknown-linux-gnu/bat", "/crate/bat/0.2.0", web)?;
            assert_redirect("/bat/0.2.0/i686-unknown-linux-gnu/bat/", "/crate/bat/0.2.0/", web)?;
            */
            Ok(())
        })
    }

    #[test]
    fn can_view_source() {
        wrapper(|env| {
            env.fake_release()
                .name("regex")
                .version("0.3.0")
                .source_file("src/main.rs", br#"println!("definitely valid rust")"#)
                .create()
                .unwrap();

            let web = env.frontend();
            assert_success("/crate/regex/0.3.0/source/src/main.rs", web)?;
            assert_success("/crate/regex/0.3.0/source", web)?;
            assert_success("/crate/regex/0.3.0/source/src", web)?;
            assert_success("/regex/0.3.0/src/regex/main.rs.html", web)?;
            Ok(())
        })
    }

    #[test]
    // https://github.com/rust-lang/docs.rs/issues/223
    fn prereleases_are_not_considered_for_semver() {
        wrapper(|env| {
            let db = env.db();
            let version = |v| version(v, db);
            let release = |v| release(v, env);

            release("0.3.1-pre");
            for search in &["*", "newest", "latest"] {
                assert_eq!(version(Some(search)), semver("0.3.1-pre"));
            }

            release("0.3.1-alpha");
            assert_eq!(version(Some("0.3.1-alpha")), exact("0.3.1-alpha"));

            release("0.3.0");
            let three = semver("0.3.0");
            assert_eq!(version(None), three);
            // same thing but with "*"
            assert_eq!(version(Some("*")), three);
            // make sure exact matches still work
            assert_eq!(version(Some("0.3.0")), exact("0.3.0"));

            Ok(())
        });
    }

    #[test]
    fn platform_dropdown_not_shown_with_no_targets() {
        wrapper(|env| {
            release("0.1.0", env);
            let web = env.frontend();
            let text = web.get("/foo/0.1.0/foo").send()?.text()?;
            let platform = kuchiki::parse_html()
                .one(text)
                .select(r#"ul > li > a[aria-label="Platform"]"#)
                .unwrap()
                .count();
            assert_eq!(platform, 0);

            // sanity check the test is doing something
            env.fake_release()
                .name("foo")
                .version("0.2.0")
                .add_platform("x86_64-unknown-linux-musl")
                .create()?;
            let text = web.get("/foo/0.2.0/foo").send()?.text()?;
            let platform = kuchiki::parse_html()
                .one(text)
                .select(r#"ul > li > a[aria-label="Platform"]"#)
                .unwrap()
                .count();
            assert_eq!(platform, 1);
            Ok(())
        });
    }

    #[test]
    // https://github.com/rust-lang/docs.rs/issues/221
    fn yanked_crates_are_not_considered() {
        wrapper(|env| {
            let db = env.db();

            let release_id = release("0.3.0", env);
            let query = "UPDATE releases SET yanked = true WHERE id = $1 AND version = '0.3.0'";

            db.conn().query(query, &[&release_id]).unwrap();
            assert_eq!(version(None, db), None);
            assert_eq!(version(Some("0.3"), db), None);

            release("0.1.0+4.1", env);
            assert_eq!(version(Some("0.1.0+4.1"), db), exact("0.1.0+4.1"));
            assert_eq!(version(None, db), semver("0.1.0+4.1"));

            Ok(())
        });
    }

    #[test]
    // https://github.com/rust-lang/docs.rs/issues/1682
    fn prereleases_are_considered_when_others_dont_match() {
        wrapper(|env| {
            let db = env.db();

            // normal release
            release("1.0.0", env);
            // prereleases
            release("2.0.0-alpha.1", env);
            release("2.0.0-alpha.2", env);

            // STAR gives me the prod release
            assert_eq!(version(Some("*"), db), exact("1.0.0"));

            // prerelease query gives me the latest prerelease
            assert_eq!(version(Some(">=2.0.0-alpha"), db), exact("2.0.0-alpha.2"));

            Ok(())
        })
    }

    #[test]
    // vaguely related to https://github.com/rust-lang/docs.rs/issues/395
    fn metadata_has_no_effect() {
        wrapper(|env| {
            let db = env.db();

            release("0.1.0+4.1", env);
            release("0.1.1", env);
            assert_eq!(version(None, db), semver("0.1.1"));
            release("0.5.1+zstd.1.4.4", env);
            assert_eq!(version(None, db), semver("0.5.1+zstd.1.4.4"));
            assert_eq!(version(Some("0.5"), db), semver("0.5.1+zstd.1.4.4"));
            assert_eq!(
                version(Some("0.5.1+zstd.1.4.4"), db),
                exact("0.5.1+zstd.1.4.4")
            );

            Ok(())
        });
    }

    #[test]
    fn serialize_metadata() {
        let mut metadata = MetaData {
            name: "serde".to_string(),
            version: "1.0.0".to_string(),
            version_or_latest: "1.0.0".to_string(),
            description: Some("serde does stuff".to_string()),
            target_name: None,
            rustdoc_status: true,
            default_target: "x86_64-unknown-linux-gnu".to_string(),
            doc_targets: vec![
                "x86_64-unknown-linux-gnu".to_string(),
                "arm64-unknown-linux-gnu".to_string(),
            ],
            yanked: false,
            rustdoc_css_file: "rustdoc.css".to_string(),
        };

        let correct_json = json!({
            "name": "serde",
            "version": "1.0.0",
            "version_or_latest": "1.0.0",
            "description": "serde does stuff",
            "target_name": null,
            "rustdoc_status": true,
            "default_target": "x86_64-unknown-linux-gnu",
            "doc_targets": [
                "x86_64-unknown-linux-gnu",
                "arm64-unknown-linux-gnu",
            ],
            "yanked": false,
            "rustdoc_css_file": "rustdoc.css",
        });

        assert_eq!(correct_json, serde_json::to_value(&metadata).unwrap());

        metadata.target_name = Some("serde_lib_name".to_string());
        let correct_json = json!({
            "name": "serde",
            "version": "1.0.0",
            "version_or_latest": "1.0.0",
            "description": "serde does stuff",
            "target_name": "serde_lib_name",
            "rustdoc_status": true,
            "default_target": "x86_64-unknown-linux-gnu",
            "doc_targets": [
                "x86_64-unknown-linux-gnu",
                "arm64-unknown-linux-gnu",
            ],
            "yanked": false,
            "rustdoc_css_file": "rustdoc.css",
        });

        assert_eq!(correct_json, serde_json::to_value(&metadata).unwrap());

        metadata.description = None;
        let correct_json = json!({
            "name": "serde",
            "version": "1.0.0",
            "version_or_latest": "1.0.0",
            "description": null,
            "target_name": "serde_lib_name",
            "rustdoc_status": true,
            "default_target": "x86_64-unknown-linux-gnu",
            "doc_targets": [
                "x86_64-unknown-linux-gnu",
                "arm64-unknown-linux-gnu",
            ],
            "yanked": false,
            "rustdoc_css_file": "rustdoc.css",
        });

        assert_eq!(correct_json, serde_json::to_value(&metadata).unwrap());
    }

    #[test]
    fn metadata_from_crate() {
        wrapper(|env| {
            release("0.1.0", env);
            let mut conn = env.db().conn();
            let metadata = MetaData::from_crate(&mut conn, "foo", "0.1.0", "latest");
            assert_eq!(
                metadata.unwrap(),
                MetaData {
                    name: "foo".to_string(),
                    version_or_latest: "latest".to_string(),
                    version: "0.1.0".to_string(),
                    description: Some("Fake package".to_string()),
                    target_name: Some("foo".to_string()),
                    rustdoc_status: true,
                    default_target: "x86_64-unknown-linux-gnu".to_string(),
                    doc_targets: vec![],
                    yanked: false,
                    rustdoc_css_file: "rustdoc.css".to_string(),
                },
            );
            Ok(())
        })
    }

    #[test]
    fn test_tabindex_is_present_on_topbar_crate_search_input() {
        wrapper(|env| {
            release("0.1.0", env);
            let web = env.frontend();
            let text = web.get("/foo/0.1.0/foo").send()?.text()?;
            let tabindex = kuchiki::parse_html()
                .one(text)
                .select(r#"#nav-search[tabindex="-1"]"#)
                .unwrap()
                .count();
            assert_eq!(tabindex, 1);
            Ok(())
        });
    }

    #[test]
    fn test_axum_redirect() {
        let response = axum_redirect("/something").unwrap().into_response();
        assert_eq!(response.status(), StatusCode::FOUND);
        assert_eq!(
            response.headers().get(http::header::LOCATION).unwrap(),
            "/something"
        );
        assert!(response
            .headers()
            .get(http::header::CACHE_CONTROL)
            .is_none());
        assert!(response.extensions().get::<cache::CachePolicy>().is_none());
    }

    #[test]
    fn test_axum_redirect_cached() {
        let response = axum_cached_redirect("/something", cache::CachePolicy::NoCaching)
            .unwrap()
            .into_response();
        assert_eq!(response.status(), StatusCode::FOUND);
        assert_eq!(
            response.headers().get(http::header::LOCATION).unwrap(),
            "/something"
        );
        assert!(matches!(
            response.extensions().get::<cache::CachePolicy>().unwrap(),
            cache::CachePolicy::NoCaching,
        ))
    }

    #[test_case("without_leading_slash")]
    #[test_case("//with_double_leading_slash")]
    fn test_axum_redirect_failure(path: &str) {
        assert!(axum_redirect(path).is_err());
        assert!(axum_cached_redirect(path, cache::CachePolicy::NoCaching).is_err());
    }
}
