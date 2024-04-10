//! Web interface of docs.rs

pub mod page;
// mod tmp;

use crate::db::types::BuildStatus;
use crate::utils::get_correct_docsrs_style_file;
use crate::utils::report_error;
use crate::web::page::templates::filters;
use anyhow::{anyhow, bail, Context as _, Result};
use axum_extra::middleware::option_layer;
use rinja::Template;
use serde_json::Value;
use tracing::{info, instrument};

mod build_details;
mod builds;
pub(crate) mod cache;
pub(crate) mod crate_details;
mod csp;
pub(crate) mod error;
mod extractors;
mod features;
mod file;
mod headers;
mod highlight;
mod markdown;
pub(crate) mod metrics;
mod releases;
mod routes;
pub(crate) mod rustdoc;
mod sitemap;
mod source;
mod statics;
mod status;

use crate::{impl_axum_webpage, Context};
use anyhow::Error;
use axum::{
    extract::{Extension, MatchedPath, Request as AxumRequest},
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
use semver::{Version, VersionReq};
use serde_with::{DeserializeFromStr, SerializeDisplay};
use std::{
    borrow::{Borrow, Cow},
    fmt::{self, Display},
    net::{IpAddr, Ipv4Addr, SocketAddr},
    str::FromStr,
    sync::Arc,
};
use tower::ServiceBuilder;
use tower_http::{catch_panic::CatchPanicLayer, timeout::TimeoutLayer, trace::TraceLayer};
use url::form_urlencoded;

use self::crate_details::Release;

// from https://github.com/servo/rust-url/blob/master/url/src/parser.rs
// and https://github.com/tokio-rs/axum/blob/main/axum-extra/src/lib.rs
const FRAGMENT: &AsciiSet = &CONTROLS.add(b' ').add(b'"').add(b'<').add(b'>').add(b'`');
const PATH: &AsciiSet = &FRAGMENT.add(b'#').add(b'?').add(b'{').add(b'}');

pub(crate) fn encode_url_path(path: &str) -> String {
    utf8_percent_encode(path, PATH).to_string()
}

const DEFAULT_BIND: SocketAddr = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(0, 0, 0, 0)), 3000);

/// Represents a version identifier in a request in the original state.
/// Can be an exact version, a semver requirement, or the string "latest".
#[derive(Debug, Default, Clone, PartialEq, Eq, SerializeDisplay, DeserializeFromStr)]
pub(crate) enum ReqVersion {
    Exact(Version),
    Semver(VersionReq),
    #[default]
    Latest,
}

impl ReqVersion {
    pub(crate) fn is_latest(&self) -> bool {
        matches!(self, ReqVersion::Latest)
    }
}

impl Display for ReqVersion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ReqVersion::Exact(version) => version.fmt(f),
            ReqVersion::Semver(version_req) => version_req.fmt(f),
            ReqVersion::Latest => write!(f, "latest"),
        }
    }
}

impl FromStr for ReqVersion {
    type Err = semver::Error;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if s == "latest" {
            Ok(ReqVersion::Latest)
        } else if let Ok(version) = Version::parse(s) {
            Ok(ReqVersion::Exact(version))
        } else if s.is_empty() || s == "newest" {
            Ok(ReqVersion::Semver(VersionReq::STAR))
        } else {
            VersionReq::parse(s).map(ReqVersion::Semver)
        }
    }
}

#[derive(Debug)]
pub(crate) struct MatchedRelease {
    /// crate name
    pub name: String,

    /// The crate name that was found when attempting to load a crate release.
    /// `match_version` will attempt to match a provided crate name against similar crate names with
    /// dashes (`-`) replaced with underscores (`_`) and vice versa.
    pub corrected_name: Option<String>,

    /// what kind of version did we get in the request? ("latest", semver, exact)
    pub req_version: ReqVersion,

    /// the matched release
    pub release: crate_details::Release,

    /// all releases since we have them anyways and so we can pass them to CrateDetails
    pub(crate) all_releases: Vec<crate_details::Release>,
}

impl MatchedRelease {
    fn assume_exact_name(self) -> Result<Self, AxumNope> {
        if self.corrected_name.is_none() {
            Ok(self)
        } else {
            Err(AxumNope::CrateNotFound)
        }
    }

    fn into_exactly_named(self) -> Self {
        if let Some(corrected_name) = self.corrected_name {
            Self {
                name: corrected_name.to_owned(),
                corrected_name: None,
                ..self
            }
        } else {
            self
        }
    }

    fn into_exactly_named_or_else<F>(self, f: F) -> Result<Self, AxumNope>
    where
        F: FnOnce(&str, &ReqVersion) -> AxumNope,
    {
        if let Some(corrected_name) = self.corrected_name {
            Err(f(&corrected_name, &self.req_version))
        } else {
            Ok(self)
        }
    }

    /// Canonicalize the the version from the request
    ///
    /// Mainly:
    /// * "newest"/"*" or empty -> "latest" in the URL
    /// * any other semver requirement -> specific version in the URL
    fn into_canonical_req_version(self) -> Self {
        match self.req_version {
            ReqVersion::Exact(_) | ReqVersion::Latest => self,
            ReqVersion::Semver(version_req) => {
                if version_req == VersionReq::STAR {
                    Self {
                        req_version: ReqVersion::Latest,
                        ..self
                    }
                } else {
                    Self {
                        req_version: ReqVersion::Exact(self.release.version.clone()),
                        ..self
                    }
                }
            }
        }
    }

    /// translate this MatchRelease into a specific semver::Version while canonicalizing the
    /// version specification.
    fn into_canonical_req_version_or_else<F>(self, f: F) -> Result<Self, AxumNope>
    where
        F: FnOnce(&ReqVersion) -> AxumNope,
    {
        let original_req_version = self.req_version.clone();
        let canonicalized = self.into_canonical_req_version();

        if canonicalized.req_version == original_req_version {
            Ok(canonicalized)
        } else {
            Err(f(&canonicalized.req_version))
        }
    }

    fn into_version(self) -> Version {
        self.release.version
    }

    fn version(&self) -> &Version {
        &self.release.version
    }

    fn id(&self) -> i32 {
        self.release.id
    }

    fn build_status(&self) -> BuildStatus {
        self.release.build_status
    }

    fn rustdoc_status(&self) -> bool {
        self.release.rustdoc_status.unwrap_or(false)
    }

    fn target_name(&self) -> Option<&str> {
        self.release.target_name.as_deref()
    }

    fn is_latest_url(&self) -> bool {
        matches!(self.req_version, ReqVersion::Latest)
    }
}

fn semver_match<'a, F: Fn(&Release) -> bool>(
    releases: &'a [Release],
    req: &VersionReq,
    filter: F,
) -> Option<&'a Release> {
    // first try standard semver match using `VersionReq::match`, should handle most cases.
    if let Some(release) = releases
        .iter()
        .filter(|release| filter(release))
        .find(|release| req.matches(&release.version))
    {
        Some(release)
    } else if req == &VersionReq::STAR {
        // semver `*` does not match pre-releases.
        // So when we only have pre-releases, `VersionReq::STAR` would lead to an
        // empty result.
        // In this case we just return the latest latest prerelase instead of nothing.
        return releases.iter().find(|release| filter(release));
    } else {
        None
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
#[instrument(skip(conn))]
async fn match_version(
    conn: &mut sqlx::PgConnection,
    name: &str,
    input_version: &ReqVersion,
) -> Result<MatchedRelease, AxumNope> {
    let (crate_id, corrected_name) = {
        let row = sqlx::query!(
            "SELECT id, name
             FROM crates
             WHERE normalize_crate_name(name) = normalize_crate_name($1)",
            name,
        )
        .fetch_optional(&mut *conn)
        .await
        .context("error fetching crate")?
        .ok_or(AxumNope::CrateNotFound)?;

        if row.name != name {
            (row.id, Some(row.name))
        } else {
            (row.id, None)
        }
    };

    // first load and parse all versions of this crate,
    // `releases_for_crate` is already sorted, newest version first.
    let releases = crate_details::releases_for_crate(conn, crate_id)
        .await
        .context("error fetching releases for crate")?;

    if releases.is_empty() {
        return Err(AxumNope::CrateNotFound);
    }

    let req_semver: VersionReq = match input_version {
        ReqVersion::Exact(parsed_req_version) => {
            if let Some(release) = releases
                .iter()
                .find(|release| &release.version == parsed_req_version)
            {
                return Ok(MatchedRelease {
                    name: name.to_owned(),
                    corrected_name,
                    req_version: input_version.clone(),
                    release: release.clone(),
                    all_releases: releases,
                });
            }

            if let Ok(version_req) = VersionReq::parse(&parsed_req_version.to_string()) {
                // when we don't find a release with exact version,
                // we try to interpret it as a semver requirement.
                // A normal semver version ("1.2.3") is equivalent to a caret semver requirement.
                version_req
            } else {
                return Err(AxumNope::VersionNotFound);
            }
        }
        ReqVersion::Latest => VersionReq::STAR,
        ReqVersion::Semver(version_req) => version_req.clone(),
    };

    // when matching semver requirements,
    // we generally only want to look at non-yanked releases,
    // excluding releases which just contain in-progress builds
    if let Some(release) = semver_match(&releases, &req_semver, |r: &Release| {
        r.build_status != BuildStatus::InProgress && (r.yanked.is_none() || r.yanked == Some(false))
    }) {
        return Ok(MatchedRelease {
            name: name.to_owned(),
            corrected_name,
            req_version: input_version.clone(),
            release: release.clone(),
            all_releases: releases,
        });
    }

    // when we don't find any match with "normal" releases, we also look into in-progress releases
    if let Some(release) = semver_match(&releases, &req_semver, |r: &Release| {
        r.yanked.is_none() || r.yanked == Some(false)
    }) {
        return Ok(MatchedRelease {
            name: name.to_owned(),
            corrected_name,
            req_version: input_version.clone(),
            release: release.clone(),
            all_releases: releases,
        });
    }

    // Since we return with a CrateNotFound earlier if the db reply is empty,
    // we know that versions were returned but none satisfied the version requirement.
    // This can only happen when all versions are yanked.
    Err(AxumNope::VersionNotFound)
}

async fn log_timeouts_to_sentry(req: AxumRequest, next: Next) -> AxumResponse {
    let uri = req.uri().clone();

    let response = next.run(req).await;

    if response.status() == StatusCode::REQUEST_TIMEOUT {
        tracing::error!(?uri, "request timeout");
    }

    response
}

async fn set_sentry_transaction_name_from_axum_route(
    request: AxumRequest,
    next: Next,
) -> AxumResponse {
    let route_name = if let Some(path) = request.extensions().get::<MatchedPath>() {
        path.as_str()
    } else {
        request.uri().path()
    };

    sentry::configure_scope(|scope| {
        scope.set_transaction(Some(route_name));
    });

    next.run(request).await
}

fn apply_middleware(
    router: AxumRouter,
    context: &dyn Context,
    template_data: Option<Arc<TemplateData>>,
) -> Result<AxumRouter> {
    let config = context.config()?;
    let has_templates = template_data.is_some();
    let runtime = context.runtime()?;
    let async_storage = runtime.block_on(context.async_storage())?;
    let build_queue = context.build_queue()?;

    Ok(router.layer(
        ServiceBuilder::new()
            .layer(TraceLayer::new_for_http())
            .layer(sentry_tower::NewSentryLayer::new_from_top())
            .layer(sentry_tower::SentryHttpLayer::with_transaction())
            .layer(middleware::from_fn(
                set_sentry_transaction_name_from_axum_route,
            ))
            .layer(CatchPanicLayer::new())
            .layer(option_layer(
                config
                    .report_request_timeouts
                    .then_some(middleware::from_fn(log_timeouts_to_sentry)),
            ))
            .layer(option_layer(config.request_timeout.map(TimeoutLayer::new)))
            .layer(Extension(context.pool()?))
            .layer(Extension(build_queue))
            .layer(Extension(context.service_metrics()?))
            .layer(Extension(context.instance_metrics()?))
            .layer(Extension(context.config()?))
            .layer(Extension(context.storage()?))
            .layer(Extension(async_storage))
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
        match tokio::net::TcpListener::bind(axum_addr)
            .await
            .context("error binding socket for metrics web server")
        {
            Ok(listener) => {
                if let Err(err) = axum::serve(listener, metrics_axum_app)
                    .await
                    .context("error running metrics web server")
                {
                    report_error(&err);
                }
            }
            Err(err) => {
                report_error(&err);
            }
        };
    });

    Ok(())
}

#[instrument(skip_all)]
pub fn start_web_server(addr: Option<SocketAddr>, context: &dyn Context) -> Result<(), Error> {
    let template_data = Arc::new(TemplateData::new(context.config()?.render_threads)?);

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

    let app = build_axum_app(context, template_data)?.into_make_service();
    context.runtime()?.block_on(async {
        let listener = tokio::net::TcpListener::bind(axum_addr)
            .await
            .context("error binding socket for metrics web server")?;

        axum::serve(listener, app)
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
) -> Result<axum::response::Response, Error>
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
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(test, derive(serde::Serialize))]
pub(crate) struct MetaData {
    pub(crate) name: String,
    /// The exact version of the release being shown.
    pub(crate) version: Version,
    /// The version identifier in the request that was used to request this page.
    /// This might be any of the variants of `ReqVersion`, but
    /// due to a canonicalization step, it is either an Exact version, or `/latest/`
    /// most of the time.
    pub(crate) req_version: ReqVersion,
    pub(crate) description: Option<String>,
    pub(crate) target_name: Option<String>,
    pub(crate) rustdoc_status: Option<bool>,
    pub(crate) default_target: Option<String>,
    pub(crate) doc_targets: Option<Vec<String>>,
    pub(crate) yanked: Option<bool>,
    /// CSS file to use depending on the rustdoc version used to generate this version of this
    /// crate.
    pub(crate) rustdoc_css_file: Option<String>,
}

impl MetaData {
    #[fn_error_context::context("getting metadata for {name} {version}")]
    async fn from_crate(
        conn: &mut sqlx::PgConnection,
        name: &str,
        version: &Version,
        req_version: Option<ReqVersion>,
    ) -> Result<MetaData> {
        let row = sqlx::query!(
            r#"SELECT
                crates.name,
                releases.version,
                releases.description,
                releases.target_name,
                releases.rustdoc_status,
                releases.default_target,
                releases.doc_targets,
                releases.yanked,
                builds.rustc_version as "rustc_version?"
            FROM releases
            INNER JOIN crates ON crates.id = releases.crate_id
            LEFT JOIN LATERAL (
                SELECT * FROM builds
                WHERE builds.rid = releases.id
                ORDER BY builds.build_time
                DESC LIMIT 1
            ) AS builds ON true
            WHERE crates.name = $1 AND releases.version = $2"#,
            name,
            version.to_string(),
        )
        .fetch_one(&mut *conn)
        .await
        .context("error fetching crate metadata")?;

        Ok(MetaData {
            name: row.name,
            version: version.clone(),
            req_version: req_version.unwrap_or_else(|| ReqVersion::Exact(version.clone())),
            description: row.description,
            target_name: row.target_name,
            rustdoc_status: row.rustdoc_status,
            default_target: row.default_target,
            doc_targets: row.doc_targets.map(MetaData::parse_doc_targets),
            yanked: row.yanked,
            rustdoc_css_file: row
                .rustc_version
                .as_deref()
                .map(get_correct_docsrs_style_file)
                .transpose()?,
        })
    }

    fn parse_doc_targets(targets: Value) -> Vec<String> {
        let mut targets: Vec<_> = targets
            .as_array()
            .map(|array| {
                array
                    .iter()
                    .filter_map(|item| item.as_str().map(|s| s.to_owned()))
                    .collect()
            })
            .unwrap_or_default();
        targets.sort_unstable();
        targets
    }

    fn target_name_url(&self) -> String {
        if let Some(ref target_name) = self.target_name {
            format!("{target_name}/index.html")
        } else {
            String::new()
        }
    }

    pub(crate) fn doc_targets(&self) -> Option<&[String]> {
        self.doc_targets.as_deref()
    }
}

#[derive(Template)]
#[template(path = "error.html")]
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct AxumErrorPage {
    /// The title of the page
    pub title: &'static str,
    /// The error message, displayed as a description
    pub message: Cow<'static, str>,
    pub status: StatusCode,
    pub csp_nonce: String,
}

impl_axum_webpage! {
    AxumErrorPage,
    status = |err| err.status,
}

#[cfg(test)]
mod test {
    use super::*;
    use crate::{docbuilder::DocCoverage, test::*};
    use kuchikiki::traits::TendrilSink;
    use serde_json::json;
    use test_case::test_case;

    async fn release(version: &str, env: &TestEnvironment) -> i32 {
        env.async_fake_release()
            .await
            .name("foo")
            .version(version)
            .create_async()
            .await
            .unwrap()
    }

    async fn version(v: Option<&str>, db: &TestDatabase) -> Option<Version> {
        let mut conn = db.async_conn().await;
        let version = match_version(
            &mut conn,
            "foo",
            &ReqVersion::from_str(v.unwrap_or_default()).unwrap(),
        )
        .await
        .ok()?
        .assume_exact_name()
        .ok()?
        .into_version();
        Some(version)
    }

    #[allow(clippy::unnecessary_wraps)]
    fn semver(version: &'static str) -> Option<Version> {
        version.parse().ok()
    }

    #[allow(clippy::unnecessary_wraps)]
    fn exact(version: &'static str) -> Option<Version> {
        version.parse().ok()
    }

    fn clipboard_is_present_for_path(path: &str, web: &TestFrontend) -> bool {
        let data = web.get(path).send().unwrap().text().unwrap();
        let node = kuchikiki::parse_html().one(data);
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

            let foo_crate =
                kuchikiki::parse_html().one(web.get("/crate/foo/0.0.1").send()?.text()?);
            for (idx, value) in ["60%", "6", "10", "2", "1"].iter().enumerate() {
                assert!(
                    foo_crate
                        .select(".pure-menu-item b")
                        .unwrap()
                        .any(|e| dbg!(e.text_contents()).contains(value)),
                    "({idx}, {value:?})"
                );
            }

            let foo_doc = kuchikiki::parse_html().one(web.get("/foo/0.0.1/foo").send()?.text()?);
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
    fn double_slash_does_redirect_to_latest_version() {
        wrapper(|env| {
            env.fake_release()
                .name("bat")
                .version("0.2.0")
                .create()
                .unwrap();
            let web = env.frontend();
            assert_redirect("/bat//", "/bat/latest/bat/", web)?;
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
        async_wrapper(|env| async move {
            let db = env.async_db().await;
            let version = |v| version(v, db);
            let release = |v| release(v, &env);

            release("0.3.1-pre").await;
            for search in &["*", "newest", "latest"] {
                assert_eq!(version(Some(search)).await, semver("0.3.1-pre"));
            }

            release("0.3.1-alpha").await;
            assert_eq!(version(Some("0.3.1-alpha")).await, exact("0.3.1-alpha"));

            release("0.3.0").await;
            let three = semver("0.3.0");
            assert_eq!(version(None).await, three);
            // same thing but with "*"
            assert_eq!(version(Some("*")).await, three);
            // make sure exact matches still work
            assert_eq!(version(Some("0.3.0")).await, exact("0.3.0"));

            Ok(())
        });
    }

    #[test]
    fn platform_dropdown_not_shown_with_no_targets() {
        wrapper(|env| {
            env.runtime().block_on(release("0.1.0", env));
            let web = env.frontend();
            let text = web.get("/foo/0.1.0/foo").send()?.text()?;
            let platform = kuchikiki::parse_html()
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
            let platform = kuchikiki::parse_html()
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
        async_wrapper(|env| async move {
            let db = env.async_db().await;

            let release_id = release("0.3.0", &env).await;

            sqlx::query!(
                "UPDATE releases SET yanked = true WHERE id = $1 AND version = '0.3.0'",
                release_id
            )
            .execute(&mut *db.async_conn().await)
            .await?;

            assert_eq!(version(None, db).await, None);
            assert_eq!(version(Some("0.3"), db).await, None);

            release("0.1.0+4.1", &env).await;
            assert_eq!(version(Some("0.1.0+4.1"), db).await, exact("0.1.0+4.1"));
            assert_eq!(version(None, db).await, semver("0.1.0+4.1"));

            Ok(())
        });
    }

    #[test]
    fn in_progress_releases_are_ignored_when_others_match() {
        async_wrapper(|env| async move {
            let db = env.async_db().await;

            // normal release
            release("1.0.0", &env).await;

            // in progress release
            env.async_fake_release()
                .await
                .name("foo")
                .version("1.1.0")
                .builds(vec![
                    FakeBuild::default().build_status(BuildStatus::InProgress)
                ])
                .create_async()
                .await?;

            // STAR gives me the prod release
            assert_eq!(version(Some("*"), db).await, exact("1.0.0"));

            // exact-match query gives me the in progress release
            assert_eq!(version(Some("=1.1.0"), db).await, exact("1.1.0"));

            Ok(())
        })
    }

    #[test]
    // https://github.com/rust-lang/docs.rs/issues/1682
    fn prereleases_are_considered_when_others_dont_match() {
        async_wrapper(|env| async move {
            let db = env.async_db().await;

            // normal release
            release("1.0.0", &env).await;
            // prereleases
            release("2.0.0-alpha.1", &env).await;
            release("2.0.0-alpha.2", &env).await;

            // STAR gives me the prod release
            assert_eq!(version(Some("*"), db).await, exact("1.0.0"));

            // prerelease query gives me the latest prerelease
            assert_eq!(
                version(Some(">=2.0.0-alpha"), db).await,
                exact("2.0.0-alpha.2")
            );

            Ok(())
        })
    }

    #[test]
    // vaguely related to https://github.com/rust-lang/docs.rs/issues/395
    fn metadata_has_no_effect() {
        async_wrapper(|env| async move {
            let db = env.async_db().await;

            release("0.1.0+4.1", &env).await;
            release("0.1.1", &env).await;
            assert_eq!(version(None, db).await, semver("0.1.1"));
            release("0.5.1+zstd.1.4.4", &env).await;
            assert_eq!(version(None, db).await, semver("0.5.1+zstd.1.4.4"));
            assert_eq!(version(Some("0.5"), db).await, semver("0.5.1+zstd.1.4.4"));
            assert_eq!(
                version(Some("0.5.1+zstd.1.4.4"), db).await,
                exact("0.5.1+zstd.1.4.4")
            );

            Ok(())
        });
    }

    #[test]
    fn serialize_metadata() {
        let mut metadata = MetaData {
            name: "serde".to_string(),
            version: "1.0.0".parse().unwrap(),
            req_version: ReqVersion::Latest,
            description: Some("serde does stuff".to_string()),
            target_name: None,
            rustdoc_status: Some(true),
            default_target: Some("x86_64-unknown-linux-gnu".to_string()),
            doc_targets: Some(vec![
                "x86_64-unknown-linux-gnu".to_string(),
                "arm64-unknown-linux-gnu".to_string(),
            ]),
            yanked: Some(false),
            rustdoc_css_file: Some("rustdoc.css".to_string()),
        };

        let correct_json = json!({
            "name": "serde",
            "version": "1.0.0",
            "req_version": "latest",
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
            "req_version": "latest",
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
            "req_version": "latest",
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
        async_wrapper(|env| async move {
            release("0.1.0", &env).await;
            let mut conn = env.async_db().await.async_conn().await;
            let metadata = MetaData::from_crate(
                &mut conn,
                "foo",
                &"0.1.0".parse().unwrap(),
                Some(ReqVersion::Latest),
            )
            .await;
            assert_eq!(
                metadata.unwrap(),
                MetaData {
                    name: "foo".to_string(),
                    version: "0.1.0".parse().unwrap(),
                    req_version: ReqVersion::Latest,
                    description: Some("Fake package".to_string()),
                    target_name: Some("foo".to_string()),
                    rustdoc_status: Some(true),
                    default_target: Some("x86_64-unknown-linux-gnu".to_string()),
                    doc_targets: Some(vec![]),
                    yanked: Some(false),
                    rustdoc_css_file: Some("rustdoc.css".to_string()),
                },
            );
            Ok(())
        })
    }

    #[test]
    fn test_tabindex_is_present_on_topbar_crate_search_input() {
        wrapper(|env| {
            env.runtime().block_on(release("0.1.0", env));
            let web = env.frontend();
            let text = web.get("/foo/0.1.0/foo").send()?.text()?;
            let tabindex = kuchikiki::parse_html()
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

    #[test]
    fn test_parse_req_version_latest() {
        let req_version: ReqVersion = "latest".parse().unwrap();
        assert_eq!(req_version, ReqVersion::Latest);
        assert_eq!(req_version.to_string(), "latest");
    }

    #[test_case("1.2.3")]
    fn test_parse_req_version_exact(input: &str) {
        let req_version: ReqVersion = input.parse().unwrap();
        assert_eq!(
            req_version,
            ReqVersion::Exact(Version::parse(input).unwrap())
        );
        assert_eq!(req_version.to_string(), input);
    }

    #[test_case("^1.2.3")]
    #[test_case("*")]
    fn test_parse_req_version_semver(input: &str) {
        let req_version: ReqVersion = input.parse().unwrap();
        assert_eq!(
            req_version,
            ReqVersion::Semver(VersionReq::parse(input).unwrap())
        );
        assert_eq!(req_version.to_string(), input);
    }

    #[test_case("")]
    #[test_case("newest")]
    fn test_parse_req_version_semver_latest(input: &str) {
        let req_version: ReqVersion = input.parse().unwrap();
        assert_eq!(req_version, ReqVersion::Semver(VersionReq::STAR));
        assert_eq!(req_version.to_string(), "*")
    }

    #[test_case("/something/", "/something/")] // already valid path
    #[test_case("/something>", "/something%3E")] // something to encode
    #[test_case("/something%3E", "/something%3E")] // re-running doesn't change anything
    fn test_encode_url_path(input: &str, expected: &str) {
        assert_eq!(encode_url_path(input), expected);
    }
}
