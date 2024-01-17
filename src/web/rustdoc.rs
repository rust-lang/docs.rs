//! rustdoc handler

use crate::{
    db::Pool,
    storage::rustdoc_archive_path,
    utils,
    web::{
        axum_cached_redirect, axum_parse_uri_with_params,
        cache::CachePolicy,
        crate_details::CrateDetails,
        csp::Csp,
        encode_url_path,
        error::{AxumNope, AxumResult},
        extractors::DbConnection,
        file::File,
        match_version,
        metrics::RenderingTimesRecorder,
        page::TemplateData,
        MatchSemver, MetaData,
    },
    AsyncStorage, Config, InstanceMetrics, RUSTDOC_STATIC_STORAGE_PREFIX,
};
use anyhow::{anyhow, Context as _};
use axum::{
    extract::{Extension, Path, Query},
    http::{StatusCode, Uri},
    response::{Html, IntoResponse, Response as AxumResponse},
};
use lol_html::errors::RewritingError;
use once_cell::sync::Lazy;
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, HashMap},
    sync::Arc,
};
use tracing::{debug, error, instrument, trace};

static DOC_RUST_LANG_ORG_REDIRECTS: Lazy<HashMap<&str, &str>> = Lazy::new(|| {
    HashMap::from([
        ("alloc", "stable/alloc"),
        ("core", "stable/core"),
        ("proc_macro", "stable/proc_macro"),
        ("proc-macro", "stable/proc_macro"),
        ("std", "stable/std"),
        ("test", "stable/test"),
        ("rustc", "nightly/nightly-rustc"),
        ("rustdoc", "nightly/nightly-rustc/rustdoc"),
    ])
});

#[derive(Debug, Clone, Deserialize)]
pub(crate) struct RustdocRedirectorParams {
    name: String,
    version: Option<String>,
    target: Option<String>,
}

/// try to serve a toolchain specific asset from the legacy location.
///
/// Newer rustdoc builds use a specific subfolder on the bucket,
/// a new `static-root-path` prefix (`/-/rustdoc.static/...`), which
/// is served via our `static_asset_handler`.
///
/// The legacy location is the root, both on the bucket & the URL
/// path, which is suboptimal since the route overlaps with other routes.
///
/// See also https://github.com/rust-lang/docs.rs/pull/1889
async fn try_serve_legacy_toolchain_asset(
    storage: Arc<AsyncStorage>,
    config: Arc<Config>,
    path: impl AsRef<str>,
) -> AxumResult<AxumResponse> {
    let path = path.as_ref().to_owned();
    // FIXME: this could be optimized: when a path doesn't exist
    // in storage, we don't need to recheck on every request.
    // Existing files are returned with caching headers, so
    // are cached by the CDN.
    // If cached, it doesn't need to be invalidated,
    // since new nightly versions will always put their
    // toolchain specific resources into the new folder,
    // which is reached via the new handler.
    Ok(File::from_path(&storage, &path, &config)
        .await
        .map(IntoResponse::into_response)?)
}

/// Handler called for `/:crate` and `/:crate/:version` URLs. Automatically redirects to the docs
/// or crate details page based on whether the given crate version was successfully built.
#[instrument(skip_all)]
pub(crate) async fn rustdoc_redirector_handler(
    Path(params): Path<RustdocRedirectorParams>,
    Extension(metrics): Extension<Arc<InstanceMetrics>>,
    Extension(storage): Extension<Arc<AsyncStorage>>,
    Extension(config): Extension<Arc<Config>>,
    mut conn: DbConnection,
    Query(query_pairs): Query<HashMap<String, String>>,
    uri: Uri,
) -> AxumResult<impl IntoResponse> {
    #[instrument]
    fn redirect_to_doc(
        query_pairs: &HashMap<String, String>,
        url_str: String,
        cache_policy: CachePolicy,
        path_in_crate: Option<&str>,
    ) -> AxumResult<impl IntoResponse> {
        let mut queries: BTreeMap<String, String> = BTreeMap::new();
        if let Some(path) = path_in_crate {
            queries.insert("search".into(), path.into());
        }
        queries.extend(query_pairs.to_owned());
        trace!("redirect to doc");
        Ok(axum_cached_redirect(
            axum_parse_uri_with_params(&url_str, queries)?,
            cache_policy,
        )?)
    }

    let mut rendering_time = RenderingTimesRecorder::new(&metrics.rustdoc_redirect_rendering_times);

    // global static assets for older builds are served from the root, which ends up
    // in this handler as `params.name`.
    if let Some((_, extension)) = params.name.rsplit_once('.') {
        if ["css", "js", "png", "svg", "woff", "woff2"]
            .binary_search(&extension)
            .is_ok()
        {
            rendering_time.step("serve static asset");
            return try_serve_legacy_toolchain_asset(storage, config, params.name).await;
        }
    }

    if let Some((_, extension)) = uri.path().rsplit_once('.') {
        if extension == "ico" {
            // redirect all ico requests
            // originally from:
            // https://github.com/rust-lang/docs.rs/commit/f3848a34c391841a2516a9e6ad1f80f6f490c6d0
            return Ok(axum_cached_redirect(
                "/-/static/favicon.ico",
                CachePolicy::ForeverInCdnAndBrowser,
            )?
            .into_response());
        }
    }

    let (mut crate_name, path_in_crate) = match params.name.split_once("::") {
        Some((krate, path)) => (krate.to_string(), Some(path.to_string())),
        None => (params.name.to_string(), None),
    };

    if let Some(inner_path) = DOC_RUST_LANG_ORG_REDIRECTS.get(crate_name.as_str()) {
        return Ok(redirect_to_doc(
            &query_pairs,
            format!("https://doc.rust-lang.org/{inner_path}/"),
            CachePolicy::ForeverInCdnAndStaleInBrowser,
            path_in_crate.as_deref(),
        )?
        .into_response());
    }

    // it doesn't matter if the version that was given was exact or not, since we're redirecting
    // anyway
    rendering_time.step("match version");
    let v = match_version(&mut conn, &crate_name, params.version.as_deref()).await?;
    trace!(?v, "matched version");
    if let Some(new_name) = v.corrected_name {
        // `match_version` checked against -/_ typos, so if we have a name here we should
        // use that instead
        crate_name = new_name;
    }
    let (mut version, id) = v.version.into_parts();

    // we might get requests to crate-specific JS files here.
    if let Some(ref target) = params.target {
        if target.ends_with(".js") {
            // this URL is actually from a crate-internal path, serve it there instead
            rendering_time.step("serve JS for crate");
            let krate = CrateDetails::new(&mut conn, &crate_name, &version, &version)
                .await?
                .ok_or(AxumNope::ResourceNotFound)?;

            rendering_time.step("fetch from storage");

            match storage
                .fetch_rustdoc_file(
                    &crate_name,
                    &version,
                    krate.latest_build_id.unwrap_or(0),
                    target,
                    krate.archive_storage,
                    Some(&mut rendering_time),
                )
                .await
            {
                Ok(blob) => return Ok(File(blob).into_response()),
                Err(err) => {
                    if !matches!(err.downcast_ref(), Some(AxumNope::ResourceNotFound))
                        && !matches!(err.downcast_ref(), Some(crate::storage::PathNotFoundError))
                    {
                        debug!(?target, ?err, "got error serving file");
                    }
                    // FIXME: we sometimes still get requests for toolchain
                    // specific static assets under the crate/version/ path.
                    // This is fixed in rustdoc, but pending a rebuild for
                    // docs that were affected by this bug.
                    // https://github.com/rust-lang/docs.rs/issues/1979
                    if target.starts_with("search-") || target.starts_with("settings-") {
                        return try_serve_legacy_toolchain_asset(storage, config, target).await;
                    } else {
                        return Err(err.into());
                    }
                }
            }
        }
    }

    if let None | Some("latest") = params.version.as_deref() {
        version = "latest".to_string()
    }

    // get target name and whether it has docs
    // FIXME: This is a bit inefficient but allowing us to use less code in general
    rendering_time.step("fetch release doc status");
    let (target_name, has_docs): (String, bool) = {
        let row = sqlx::query!(
            "SELECT
                 target_name,
                 rustdoc_status
             FROM releases
             WHERE releases.id = $1",
            id,
        )
        .fetch_one(&mut *conn)
        .await?;

        (row.target_name, row.rustdoc_status)
    };

    let mut target = params.target.as_deref();
    if target == Some("index.html") || target == Some(&target_name) {
        target = None;
    }

    if has_docs {
        rendering_time.step("redirect to doc");

        let url_str = if let Some(target) = target {
            format!("/{crate_name}/{version}/{target}/{target_name}/")
        } else {
            format!("/{crate_name}/{version}/{target_name}/")
        };

        let cache = if version == "latest" {
            CachePolicy::ForeverInCdn
        } else {
            CachePolicy::ForeverInCdnAndStaleInBrowser
        };

        Ok(redirect_to_doc(
            &query_pairs,
            encode_url_path(&url_str),
            cache,
            path_in_crate.as_deref(),
        )?
        .into_response())
    } else {
        rendering_time.step("redirect to crate");
        Ok(axum_cached_redirect(
            format!("/crate/{crate_name}/{version}"),
            CachePolicy::ForeverInCdn,
        )?
        .into_response())
    }
}

#[derive(Debug, Clone, Serialize)]
struct RustdocPage {
    latest_path: String,
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
    current_target: String,
}

impl RustdocPage {
    fn into_response(
        self,
        rustdoc_html: &[u8],
        max_parse_memory: usize,
        templates: &TemplateData,
        metrics: &InstanceMetrics,
        config: &Config,
        file_path: &str,
    ) -> AxumResult<AxumResponse> {
        let is_latest_url = self.is_latest_url;

        // Build the page of documentation
        let mut ctx = tera::Context::from_serialize(self).context("error creating tera context")?;
        ctx.insert("DEFAULT_MAX_TARGETS", &crate::DEFAULT_MAX_TARGETS);

        // Extract the head and body of the rustdoc file so that we can insert it into our own html
        // while logging OOM errors from html rewriting
        let html = match utils::rewrite_lol(rustdoc_html, max_parse_memory, ctx, templates) {
            Err(RewritingError::MemoryLimitExceeded(..)) => {
                metrics.html_rewrite_ooms.inc();

                return Err(AxumNope::InternalError(
                    anyhow!(
                        "Failed to serve the rustdoc file '{}' because rewriting it surpassed the memory limit of {} bytes",
                        file_path, config.max_parse_memory,
                    )
                ));
            }
            result => result.context("error rewriting HTML")?,
        };

        Ok((
            StatusCode::OK,
            (!is_latest_url).then_some([("X-Robots-Tag", "noindex")]),
            Extension(if is_latest_url {
                CachePolicy::ForeverInCdn
            } else {
                CachePolicy::ForeverInCdnAndStaleInBrowser
            }),
            Html(html),
        )
            .into_response())
    }
}

#[derive(Clone, Deserialize, Debug)]
pub(crate) struct RustdocHtmlParams {
    pub(crate) name: String,
    pub(crate) version: String,
    // both target and path are only used for matching the route.
    // The actual path is read from the request `Uri` because
    // we have some static filenames directly in the routes.
    pub(crate) target: Option<String>,
    pub(crate) path: Option<String>,
}

/// Serves documentation generated by rustdoc.
///
/// This includes all HTML files for an individual crate, as well as the `search-index.js`, which is
/// also crate-specific.
#[allow(clippy::too_many_arguments)]
#[instrument(skip_all)]
pub(crate) async fn rustdoc_html_server_handler(
    Path(params): Path<RustdocHtmlParams>,
    Extension(metrics): Extension<Arc<InstanceMetrics>>,
    Extension(templates): Extension<Arc<TemplateData>>,
    Extension(pool): Extension<Pool>,
    Extension(storage): Extension<Arc<AsyncStorage>>,
    Extension(config): Extension<Arc<Config>>,
    Extension(csp): Extension<Arc<Csp>>,
    uri: Uri,
) -> AxumResult<AxumResponse> {
    let mut rendering_time = RenderingTimesRecorder::new(&metrics.rustdoc_rendering_times);

    // since we directly use the Uri-path and not the extracted params from the router,
    // we have to percent-decode the string here.
    let original_path = percent_encoding::percent_decode(uri.path().as_bytes())
        .decode_utf8()
        .map_err(|_| AxumNope::BadRequest)?;

    let mut req_path: Vec<&str> = original_path.split('/').collect();
    // Remove the empty start, the name and the version from the path
    req_path.drain(..3).for_each(drop);

    // Pages generated by Rustdoc are not ready to be served with a CSP yet.
    csp.suppress(true);

    // Convenience function to allow for easy redirection
    #[instrument]
    fn redirect(
        name: &str,
        vers: &str,
        path: &[&str],
        cache_policy: CachePolicy,
    ) -> AxumResult<AxumResponse> {
        trace!("redirect");
        // Format and parse the redirect url
        Ok(axum_cached_redirect(
            encode_url_path(&format!("/{}/{}/{}", name, vers, path.join("/"))),
            cache_policy,
        )?
        .into_response())
    }

    trace!("match version");
    rendering_time.step("match version");

    let mut conn = pool.get_async().await?;

    // Check the database for releases with the requested version while doing the following:
    // * If no matching releases are found, return a 404 with the underlying error
    // Then:
    // * If both the name and the version are an exact match, return the version of the crate.
    // * If there is an exact match, but the requested crate name was corrected (dashes vs. underscores), redirect to the corrected name.
    // * If there is a semver (but not exact) match, redirect to the exact version.
    let release_found = match_version(&mut conn, &params.name, Some(&params.version)).await?;
    trace!(?release_found, "found release");

    let (version, version_or_latest, is_latest_url) = match release_found.version {
        MatchSemver::Exact((version, _)) => {
            // Redirect when the requested crate name isn't correct
            if let Some(name) = release_found.corrected_name {
                return redirect(&name, &version, &req_path, CachePolicy::NoCaching);
            }

            (version.clone(), version, false)
        }

        MatchSemver::Latest((version, _)) => {
            // Redirect when the requested crate name isn't correct
            if let Some(name) = release_found.corrected_name {
                return redirect(&name, "latest", &req_path, CachePolicy::NoCaching);
            }

            (version, "latest".to_string(), true)
        }

        // Redirect when the requested version isn't correct
        MatchSemver::Semver((v, _)) => {
            // to prevent cloudfront caching the wrong artifacts on URLs with loose semver
            // versions, redirect the browser to the returned version instead of loading it
            // immediately
            return redirect(&params.name, &v, &req_path, CachePolicy::ForeverInCdn);
        }
    };

    trace!("crate details");
    rendering_time.step("crate details");

    // Get the crate's details from the database
    // NOTE: we know this crate must exist because we just checked it above (or else `match_version` is buggy)
    let krate = CrateDetails::new(&mut conn, &params.name, &version, &version_or_latest)
        .await?
        .ok_or(AxumNope::ResourceNotFound)?;

    if !krate.rustdoc_status {
        rendering_time.step("redirect to crate");
        return Ok(axum_cached_redirect(
            format!("/crate/{}/{}", params.name, version_or_latest),
            CachePolicy::ForeverInCdn,
        )?
        .into_response());
    }

    // if visiting the full path to the default target, remove the target from the path
    // expects a req_path that looks like `[/:target]/.*`
    if req_path.first().copied() == Some(&krate.metadata.default_target) {
        return redirect(
            &params.name,
            &version_or_latest,
            &req_path[1..],
            CachePolicy::ForeverInCdn,
        );
    }

    // Create the path to access the file from
    let mut storage_path = req_path.join("/");
    if storage_path.ends_with('/') {
        req_path.pop(); // get rid of empty string
        storage_path.push_str("index.html");
        req_path.push("index.html");
    }

    trace!(?storage_path, ?req_path, "try fetching from storage");

    // record the data-fetch step
    // until we re-add it below inside `fetch_rustdoc_file`
    rendering_time.step("fetch from storage");

    // Attempt to load the file from the database
    let blob = match storage
        .fetch_rustdoc_file(
            &params.name,
            &version,
            krate.latest_build_id.unwrap_or(0),
            &storage_path,
            krate.archive_storage,
            Some(&mut rendering_time),
        )
        .await
    {
        Ok(file) => file,
        Err(err) => {
            if !matches!(err.downcast_ref(), Some(AxumNope::ResourceNotFound))
                && !matches!(err.downcast_ref(), Some(crate::storage::PathNotFoundError))
            {
                debug!("got error serving {}: {}", storage_path, err);
            }
            // If it fails, we try again with /index.html at the end
            storage_path.push_str("/index.html");
            req_path.push("index.html");

            return if storage
                .rustdoc_file_exists(
                    &params.name,
                    &version,
                    krate.latest_build_id.unwrap_or(0),
                    &storage_path,
                    krate.archive_storage,
                )
                .await?
            {
                redirect(
                    &params.name,
                    &version_or_latest,
                    &req_path,
                    CachePolicy::ForeverInCdn,
                )
            } else if req_path.first().map_or(false, |p| p.contains('-')) {
                // This is a target, not a module; it may not have been built.
                // Redirect to the default target and show a search page instead of a hard 404.
                Ok(axum_cached_redirect(
                    encode_url_path(&format!(
                        "/crate/{}/{}/target-redirect/{}",
                        params.name,
                        version,
                        req_path.join("/")
                    )),
                    CachePolicy::ForeverInCdn,
                )?
                .into_response())
            } else {
                if storage_path == format!("{}/index.html", krate.target_name) {
                    error!(
                        krate = params.name,
                        version,
                        original_path = original_path.as_ref(),
                        storage_path,
                        "Couldn't find crate documentation root on storage.
                        Something is wrong with the build."
                    )
                }

                Err(AxumNope::ResourceNotFound)
            };
        }
    };

    // Serve non-html files directly
    if !storage_path.ends_with(".html") {
        trace!(?storage_path, "serve asset");
        rendering_time.step("serve asset");

        // default asset caching behaviour is `Cache::ForeverInCdnAndBrowser`.
        // This is an edge-case when we serve invocation specific static assets under `/latest/`:
        // https://github.com/rust-lang/docs.rs/issues/1593
        return Ok(File(blob).into_response());
    }

    rendering_time.step("find latest path");

    let latest_release = krate.latest_release()?;

    // Get the latest version of the crate
    let latest_version = latest_release.version.to_string();
    let is_latest_version = latest_version == version;
    let is_prerelease = !(semver::Version::parse(&version)
        .with_context(|| {
            format!(
                "invalid semver in database for crate {}: {}",
                params.name, &version
            )
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
    let mut current_target = String::new();
    let target_redirect = if latest_release.build_status {
        let target = if target.is_empty() {
            current_target = krate.metadata.default_target.clone();
            &krate.metadata.default_target
        } else {
            current_target = target.to_owned();
            target
        };
        format!("/target-redirect/{target}/{inner_path}")
    } else {
        "".to_string()
    };

    let query_string = if let Some(query) = uri.query() {
        format!("?{query}")
    } else {
        "".to_string()
    };

    let permalink_path = format!(
        "/{}/{}/{}{}",
        params.name, latest_version, inner_path, query_string
    );

    let latest_path = format!(
        "/crate/{}/latest{}{}",
        params.name, target_redirect, query_string
    );

    metrics
        .recently_accessed_releases
        .record(krate.crate_id, krate.release_id, target);

    let target = if target.is_empty() {
        String::new()
    } else {
        format!("{target}/")
    };

    rendering_time.step("rewrite html");

    // Build the page of documentation,
    templates
        .render_in_threadpool({
            let metrics = metrics.clone();
            move |templates| {
                let metadata = krate.metadata.clone();
                Ok(RustdocPage {
                    latest_path,
                    permalink_path,
                    latest_version,
                    target,
                    inner_path,
                    is_latest_version,
                    is_latest_url,
                    is_prerelease,
                    metadata,
                    krate,
                    current_target,
                }
                .into_response(
                    &blob.content,
                    config.max_parse_memory,
                    templates,
                    &metrics,
                    &config,
                    &storage_path,
                ))
            }
        })
        .await?
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
fn path_for_version(
    file_path: &[&str],
    crate_details: &CrateDetails,
) -> (String, HashMap<String, String>) {
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
    let path = if platform.is_empty() {
        format!("{target_name}/")
    } else {
        format!("{platform}/{target_name}/")
    };

    let query_params = search_item
        .map(|i| HashMap::from_iter([("search".into(), i.into())]))
        .unwrap_or_default();

    (path, query_params)
}

pub(crate) async fn target_redirect_handler(
    Path((name, version, req_path)): Path<(String, String, String)>,
    mut conn: DbConnection,
    Extension(storage): Extension<Arc<AsyncStorage>>,
) -> AxumResult<impl IntoResponse> {
    let release_found = match_version(&mut conn, &name, Some(&version)).await?;
    let (version, version_or_latest, is_latest_url) = match release_found.version {
        MatchSemver::Exact((version, _)) => (version.clone(), version, false),
        MatchSemver::Latest((version, _)) => (version, "latest".to_string(), true),
        // semver matching not supported here
        MatchSemver::Semver(_) => return Err(AxumNope::VersionNotFound),
    };

    let crate_details = CrateDetails::new(&mut conn, &name, &version, &version_or_latest)
        .await?
        .ok_or(AxumNope::VersionNotFound)?;

    // We're trying to find the storage location
    // for the requested path in the target-redirect.
    // *path always contains the target,
    // here we are dropping it when it's the
    // default target,
    // and add `/index.html` if we request
    // a folder.
    let storage_location_for_path = {
        let mut pieces: Vec<_> = req_path.split('/').map(str::to_owned).collect();

        if let Some(target) = pieces.first() {
            if target == &crate_details.metadata.default_target {
                pieces.remove(0);
            }
        }

        if let Some(last) = pieces.last_mut() {
            if last.is_empty() {
                *last = "index.html".to_string();
            }
        }

        pieces.join("/")
    };

    let (redirect_path, query_args) = if storage
        .rustdoc_file_exists(
            &name,
            &version,
            crate_details.latest_build_id.unwrap_or(0),
            &storage_location_for_path,
            crate_details.archive_storage,
        )
        .await?
    {
        // Simple case: page exists in the other target & version, so just change these
        (storage_location_for_path, HashMap::new())
    } else {
        let pieces: Vec<_> = storage_location_for_path.split('/').collect();
        path_for_version(&pieces, &crate_details)
    };

    Ok(axum_cached_redirect(
        axum_parse_uri_with_params(
            &encode_url_path(&format!("/{name}/{version_or_latest}/{redirect_path}")),
            query_args,
        )?,
        if is_latest_url {
            CachePolicy::ForeverInCdn
        } else {
            CachePolicy::ForeverInCdnAndStaleInBrowser
        },
    )?)
}

#[derive(Deserialize, Debug)]
pub(crate) struct BadgeQueryParams {
    version: Option<String>,
}

#[instrument]
pub(crate) async fn badge_handler(
    Path(name): Path<String>,
    Query(query): Query<BadgeQueryParams>,
) -> AxumResult<impl IntoResponse> {
    let version = query.version.unwrap_or_else(|| "latest".to_string());

    let url = url::Url::parse(&format!("https://img.shields.io/docsrs/{name}/{version}"))
        .context("could not parse URL")?;

    Ok((
        StatusCode::MOVED_PERMANENTLY,
        [(http::header::LOCATION, url.to_string())],
        Extension(CachePolicy::ForeverInCdnAndBrowser),
    ))
}

pub(crate) async fn download_handler(
    Path((name, req_version)): Path<(String, String)>,
    mut conn: DbConnection,
    Extension(storage): Extension<Arc<AsyncStorage>>,
    Extension(config): Extension<Arc<Config>>,
) -> AxumResult<impl IntoResponse> {
    let (version, _) = match_version(&mut conn, &name, Some(&req_version))
        .await?
        .exact_name_only()?
        .into_parts();

    let archive_path = rustdoc_archive_path(&name, &version);

    // not all archives are set for public access yet, so we check if
    // the access is set and fix it if needed.
    let archive_is_public = match storage
        .get_public_access(&archive_path)
        .await
        .context("reading public access for archive")
    {
        Ok(is_public) => is_public,
        Err(err) => {
            if matches!(err.downcast_ref(), Some(crate::storage::PathNotFoundError)) {
                return Err(AxumNope::ResourceNotFound);
            } else {
                return Err(AxumNope::InternalError(err));
            }
        }
    };

    if !archive_is_public {
        storage.set_public_access(&archive_path, true).await?;
    }

    Ok(super::axum_cached_redirect(
        format!("{}/{}", config.s3_static_root_path, archive_path),
        CachePolicy::ForeverInCdn,
    )?)
}

/// Serves shared resources used by rustdoc-generated documentation.
///
/// This serves files from S3, and is pointed to by the `--static-root-path` flag to rustdoc.
pub(crate) async fn static_asset_handler(
    Path(path): Path<String>,
    Extension(storage): Extension<Arc<AsyncStorage>>,
    Extension(config): Extension<Arc<Config>>,
) -> AxumResult<impl IntoResponse> {
    let storage_path = format!("{RUSTDOC_STATIC_STORAGE_PREFIX}{path}");

    Ok(File::from_path(&storage, &storage_path, &config).await?)
}

#[cfg(test)]
mod test {
    use crate::{test::*, web::cache::CachePolicy, Config};
    use anyhow::Context;
    use kuchikiki::traits::TendrilSink;
    use reqwest::{blocking::ClientBuilder, redirect, StatusCode};
    use std::collections::BTreeMap;
    use test_case::test_case;
    use tracing::info;

    fn try_latest_version_redirect(
        path: &str,
        web: &TestFrontend,
        config: &Config,
    ) -> Result<Option<String>, anyhow::Error> {
        assert_success(path, web)?;
        let response = web.get(path).send()?;
        assert_cache_control(
            &response,
            CachePolicy::ForeverInCdnAndStaleInBrowser,
            config,
        );
        let data = response.text()?;
        info!("fetched path {} and got content {}\nhelp: if this is missing the header, remember to add <html><head></head><body></body></html>", path, data);
        let dom = kuchikiki::parse_html().one(data);

        if let Some(elem) = dom
            .select("form > ul > li > a.warn")
            .expect("invalid selector")
            .next()
        {
            let link = elem.attributes.borrow().get("href").unwrap().to_string();
            assert_success_cached(&link, web, CachePolicy::ForeverInCdn, config)?;
            Ok(Some(link))
        } else {
            Ok(None)
        }
    }

    fn latest_version_redirect(
        path: &str,
        web: &TestFrontend,
        config: &Config,
    ) -> Result<String, anyhow::Error> {
        try_latest_version_redirect(path, web, config)?
            .with_context(|| anyhow::anyhow!("no redirect found for {}", path))
    }

    #[test_case(true)]
    #[test_case(false)]
    // https://github.com/rust-lang/docs.rs/issues/2313
    fn help_html(archive_storage: bool) {
        wrapper(|env| {
            env.fake_release()
                .name("krate")
                .version("0.1.0")
                .archive_storage(archive_storage)
                .rustdoc_file("help.html")
                .create()?;
            let web = env.frontend();
            assert_success_cached(
                "/krate/0.1.0/help.html",
                web,
                CachePolicy::ForeverInCdnAndStaleInBrowser,
                &env.config(),
            )?;
            Ok(())
        });
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
                .rustdoc_file("scrape-examples-help.html")
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
            assert_success_cached("/", web, CachePolicy::ShortInCdnAndBrowser, &env.config())?;
            assert_success_cached(
                "/crate/buggy/0.1.0/",
                web,
                CachePolicy::ForeverInCdnAndStaleInBrowser,
                &env.config(),
            )?;
            assert_success_cached(
                "/buggy/0.1.0/directory_1/index.html",
                web,
                CachePolicy::ForeverInCdnAndStaleInBrowser,
                &env.config(),
            )?;
            assert_success_cached(
                "/buggy/0.1.0/directory_2.html/index.html",
                web,
                CachePolicy::ForeverInCdnAndStaleInBrowser,
                &env.config(),
            )?;
            assert_success_cached(
                "/buggy/0.1.0/directory_3/.gitignore",
                web,
                CachePolicy::ForeverInCdnAndBrowser,
                &env.config(),
            )?;
            assert_success_cached(
                "/buggy/0.1.0/settings.html",
                web,
                CachePolicy::ForeverInCdnAndStaleInBrowser,
                &env.config(),
            )?;
            assert_success_cached(
                "/buggy/0.1.0/scrape-examples-help.html",
                web,
                CachePolicy::ForeverInCdnAndStaleInBrowser,
                &env.config(),
            )?;
            assert_success_cached(
                "/buggy/0.1.0/all.html",
                web,
                CachePolicy::ForeverInCdnAndStaleInBrowser,
                &env.config(),
            )?;
            assert_success_cached(
                "/buggy/0.1.0/directory_4/empty_file_no_ext",
                web,
                CachePolicy::ForeverInCdnAndBrowser,
                &env.config(),
            )?;
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
            assert_success_cached(
                base,
                web,
                CachePolicy::ForeverInCdnAndStaleInBrowser,
                &env.config(),
            )?;
            assert_redirect_cached(
                "/dummy/0.1.0/x86_64-unknown-linux-gnu/dummy/",
                base,
                CachePolicy::ForeverInCdn,
                web,
                &env.config(),
            )?;

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
            assert_cache_control(&resp, CachePolicy::ForeverInCdn, &env.config());
            assert!(resp.url().as_str().ends_with("/dummy/latest/dummy/"));
            let body = String::from_utf8(resp.bytes().unwrap().to_vec()).unwrap();
            assert!(body.contains("<a href=\"/crate/dummy/latest/source/\""));
            assert!(body.contains("<a href=\"/crate/dummy/latest\""));
            assert!(body.contains("<a href=\"/dummy/0.1.0/dummy/index.html\""));
            Ok(())
        })
    }

    #[test]
    fn cache_headers_on_version() {
        wrapper(|env| {
            env.override_config(|config| {
                config.cache_control_stale_while_revalidate = Some(2592000);
            });

            env.fake_release()
                .name("dummy")
                .version("0.1.0")
                .archive_storage(true)
                .rustdoc_file("dummy/index.html")
                .create()?;

            let web = env.frontend();

            {
                let resp = web.get("/dummy/latest/dummy/").send()?;
                assert_cache_control(&resp, CachePolicy::ForeverInCdn, &env.config());
            }

            {
                let resp = web.get("/dummy/0.1.0/dummy/").send()?;
                assert_cache_control(
                    &resp,
                    CachePolicy::ForeverInCdnAndStaleInBrowser,
                    &env.config(),
                );
            }
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
            let redirect = latest_version_redirect("/dummy/0.1.0/dummy/", web, &env.config())?;
            assert_eq!(
                redirect,
                "/crate/dummy/latest/target-redirect/x86_64-unknown-linux-gnu/dummy/index.html"
            );

            // check it keeps the subpage
            let redirect = latest_version_redirect("/dummy/0.1.0/dummy/blah/", web, &env.config())?;
            assert_eq!(
                redirect,
                "/crate/dummy/latest/target-redirect/x86_64-unknown-linux-gnu/dummy/blah/index.html"
            );
            let redirect =
                latest_version_redirect("/dummy/0.1.0/dummy/blah/blah.html", web, &env.config())?;
            assert_eq!(
                redirect,
                "/crate/dummy/latest/target-redirect/x86_64-unknown-linux-gnu/dummy/blah/blah.html"
            );

            // check it also works for deleted pages
            let redirect = latest_version_redirect(
                "/dummy/0.1.0/dummy/struct.will-be-deleted.html",
                web,
                &env.config(),
            )?;
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

            let redirect = latest_version_redirect(
                "/dummy/0.1.0/x86_64-pc-windows-msvc/dummy",
                web,
                &env.config(),
            )?;
            assert_eq!(
                redirect,
                "/crate/dummy/latest/target-redirect/x86_64-pc-windows-msvc/dummy/index.html"
            );

            let redirect = latest_version_redirect(
                "/dummy/0.1.0/x86_64-pc-windows-msvc/dummy/",
                web,
                &env.config(),
            )?;
            assert_eq!(
                redirect,
                "/crate/dummy/latest/target-redirect/x86_64-pc-windows-msvc/dummy/index.html"
            );

            let redirect = latest_version_redirect(
                "/dummy/0.1.0/x86_64-pc-windows-msvc/dummy/struct.Blah.html",
                web,
                &env.config(),
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
            let redirect = latest_version_redirect("/dummy/0.1.0/dummy/", web, &env.config())?;
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
            let redirect = latest_version_redirect("/dummy/0.1.0/dummy/", web, &env.config())?;
            assert_eq!(
                redirect,
                "/crate/dummy/latest/target-redirect/x86_64-unknown-linux-gnu/dummy/index.html"
            );

            let redirect = latest_version_redirect("/dummy/0.2.1/dummy/", web, &env.config())?;
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
            Ok(kuchikiki::parse_html()
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
                println!("({last_url} -> {current_url})");
                assert_eq!(response.status(), StatusCode::MOVED_PERMANENTLY);
                last_url = response.url().to_string();
                response = frontend.get(response.url().as_str()).send().unwrap();
                current_url = Url::parse(response.headers()[reqwest::header::LOCATION].to_str()?)?;
            }
            println!("({last_url} -> {current_url})");
            assert_eq!(response.status(), StatusCode::MOVED_PERMANENTLY);
            assert_cache_control(
                &response,
                CachePolicy::ForeverInCdnAndBrowser,
                &env.config(),
            );
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
            let dom = kuchikiki::parse_html().one(data);
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
    fn test_redirect_to_latest_302() {
        wrapper(|env| {
            env.fake_release().name("dummy").version("1.0.0").create()?;
            let web = env.frontend();
            let client = ClientBuilder::new()
                .redirect(redirect::Policy::none())
                .build()
                .unwrap();
            let url = format!("http://{}/dummy", web.server_addr());
            let resp = client.get(url).send()?;
            assert_eq!(resp.status(), StatusCode::FOUND);
            assert!(resp.headers().get("Cache-Control").is_none());
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
                "/some_random_crate/latest/some_random_crate/?search=some%3A%3Apath",
                web,
            )?;
            assert_redirect(
                "/some_random_crate::some::path?go_to_first=true",
                "/some_random_crate/latest/some_random_crate/?go_to_first=true&search=some%3A%3Apath",
                web,
            )?;

            assert_redirect(
                "/std::some::path",
                "https://doc.rust-lang.org/stable/std/?search=some%3A%3Apath",
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
            let page = kuchikiki::parse_html()
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
                    kuchikiki::parse_html().one(web.get("/crate/hexponent/0.3.0").send()?.text()?);
                let selector = format!(r#"ul > li a[href="/crate/hexponent/{version}"]"#);
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
            )?;

            Ok(())
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
                    env.frontend(),
                    &env.config()
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
                    env.frontend(),
                    &env.config(),
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

    fn parse_release_links_from_menu(body: &str) -> Vec<String> {
        kuchikiki::parse_html()
            .one(body)
            .select(r#"ul > li > a"#)
            .expect("invalid selector")
            .map(|elem| elem.attributes.borrow().get("href").unwrap().to_string())
            .collect()
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
                .rustdoc_file("hexponent/something.html")
                .create()?;

            // test rustdoc pages stay on the documentation
            let releases_response = env
                .frontend()
                .get("/crate/hexponent/0.3.1/menus/releases")
                .send()?;
            assert!(releases_response.status().is_success());
            assert_cache_control(&releases_response, CachePolicy::ForeverInCdn, &env.config());
            assert_eq!(
                parse_release_links_from_menu(&releases_response.text()?),
                vec![
                    "/crate/hexponent/0.3.1/target-redirect/hexponent/index.html".to_owned(),
                    "/crate/hexponent/0.3.0/target-redirect/hexponent/index.html".to_owned(),
                ]
            );

            // test if target-redirect inludes path
            let releases_response = env
                .frontend()
                .get("/crate/hexponent/0.3.1/menus/releases/hexponent/something.html")
                .send()?;
            assert!(releases_response.status().is_success());
            assert_cache_control(&releases_response, CachePolicy::ForeverInCdn, &env.config());
            assert_eq!(
                parse_release_links_from_menu(&releases_response.text()?),
                vec![
                    "/crate/hexponent/0.3.1/target-redirect/hexponent/something.html".to_owned(),
                    "/crate/hexponent/0.3.0/target-redirect/hexponent/something.html".to_owned(),
                ]
            );

            // test /crate pages stay on /crate
            let page = kuchikiki::parse_html().one(
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

            let dom = kuchikiki::parse_html().one(
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

            let dom = kuchikiki::parse_html().one(
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
    fn noindex_nonlatest() {
        wrapper(|env| {
            env.fake_release()
                .name("dummy")
                .version("0.1.0")
                .rustdoc_file("dummy/index.html")
                .create()?;

            let web = env.frontend();

            assert!(web
                .get("/dummy/0.1.0/dummy/")
                .send()?
                .headers()
                .get("x-robots-tag")
                .unwrap()
                .to_str()
                .unwrap()
                .contains("noindex"));

            assert!(web
                .get("/dummy/latest/dummy/")
                .send()?
                .headers()
                .get("x-robots-tag")
                .is_none());
            Ok(())
        })
    }

    #[test]
    fn download_unknown_version_404() {
        wrapper(|env| {
            let web = env.frontend();

            let response = web.get("/crate/dummy/0.1.0/download").send()?;
            assert_cache_control(&response, CachePolicy::NoCaching, &env.config());
            assert_eq!(response.status(), StatusCode::NOT_FOUND);
            Ok(())
        });
    }

    #[test]
    fn download_old_storage_version_404() {
        wrapper(|env| {
            env.fake_release()
                .name("dummy")
                .version("0.1.0")
                .archive_storage(false)
                .create()?;

            let web = env.frontend();

            let response = web.get("/crate/dummy/0.1.0/download").send()?;
            assert_cache_control(&response, CachePolicy::NoCaching, &env.config());
            assert_eq!(response.status(), StatusCode::NOT_FOUND);
            Ok(())
        });
    }

    #[test]
    fn download_semver() {
        wrapper(|env| {
            env.fake_release()
                .name("dummy")
                .version("0.1.0")
                .archive_storage(true)
                .create()?;

            let web = env.frontend();

            assert_redirect_cached_unchecked(
                "/crate/dummy/0.1/download",
                "https://static.docs.rs/rustdoc/dummy/0.1.0.zip",
                CachePolicy::ForeverInCdn,
                web,
                &env.config(),
            )?;
            assert!(env.storage().get_public_access("rustdoc/dummy/0.1.0.zip")?);
            Ok(())
        });
    }

    #[test]
    fn download_specific_version() {
        wrapper(|env| {
            env.fake_release()
                .name("dummy")
                .version("0.1.0")
                .archive_storage(true)
                .create()?;

            let web = env.frontend();

            // disable public access to be sure that the handler will enable it
            env.storage()
                .set_public_access("rustdoc/dummy/0.1.0.zip", false)?;

            assert_redirect_cached_unchecked(
                "/crate/dummy/0.1.0/download",
                "https://static.docs.rs/rustdoc/dummy/0.1.0.zip",
                CachePolicy::ForeverInCdn,
                web,
                &env.config(),
            )?;
            assert!(env.storage().get_public_access("rustdoc/dummy/0.1.0.zip")?);
            Ok(())
        });
    }

    #[test]
    fn download_latest_version() {
        wrapper(|env| {
            env.fake_release()
                .name("dummy")
                .version("0.1.0")
                .archive_storage(true)
                .create()?;

            env.fake_release()
                .name("dummy")
                .version("0.2.0")
                .archive_storage(true)
                .create()?;

            let web = env.frontend();

            assert_redirect_cached_unchecked(
                "/crate/dummy/latest/download",
                "https://static.docs.rs/rustdoc/dummy/0.2.0.zip",
                CachePolicy::ForeverInCdn,
                web,
                &env.config(),
            )?;
            assert!(env.storage().get_public_access("rustdoc/dummy/0.2.0.zip")?);
            Ok(())
        });
    }

    #[test_case("search-1234.js")]
    #[test_case("settings-1234.js")]
    fn fallback_to_root_storage_for_some_js_assets(path: &str) {
        // test workaround for https://github.com/rust-lang/docs.rs/issues/1979
        wrapper(|env| {
            env.fake_release()
                .name("dummy")
                .version("0.1.0")
                .archive_storage(true)
                .create()?;

            env.storage().store_one("asset.js", *b"content")?;
            env.storage().store_one(path, *b"more_content")?;

            let web = env.frontend();

            assert_eq!(
                web.get("/dummy/0.1.0/asset.js").send()?.status(),
                StatusCode::NOT_FOUND
            );
            assert!(web.get("/asset.js").send()?.status().is_success());

            assert!(web.get(&format!("/{path}")).send()?.status().is_success());
            let response = web.get(&format!("/dummy/0.1.0/{path}")).send()?;
            assert!(response.status().is_success());
            assert_eq!(response.text()?, "more_content");

            Ok(())
        })
    }

    #[test]
    fn redirect_with_encoded_chars_in_path() {
        wrapper(|env| {
            env.fake_release()
                .name("clap")
                .version("2.24.0")
                .archive_storage(true)
                .create()?;
            let web = env.frontend();

            assert_redirect_cached_unchecked(
                "/clap/2.24.0/i686-pc-windows-gnu/clap/which%20is%20a%20part%20of%20%5B%60Display%60%5D",
                "/crate/clap/2.24.0/target-redirect/i686-pc-windows-gnu/clap/which%20is%20a%20part%20of%20[%60Display%60]/index.html",
                CachePolicy::ForeverInCdn,
                web,
                &env.config(),
            )?;

            Ok(())
        })
    }

    #[test]
    fn search_with_encoded_chars_in_path() {
        wrapper(|env| {
            env.fake_release()
                .name("clap")
                .version("2.24.0")
                .archive_storage(true)
                .create()?;
            let web = env.frontend();

            assert_redirect_cached_unchecked(
                "/clap/latest/clapproc%20macro%20%60Parser%60%20not%20expanded:%20Cannot%20create%20expander%20for",
                "/clap/latest/clapproc%20macro%20%60Parser%60%20not%20expanded:%20Cannot%20create%20expander%20for/clap/",
                CachePolicy::ForeverInCdn,
                web,
                &env.config(),
            )?;

            Ok(())
        })
    }

    #[test_case("/something/1.2.3/some_path/", "/crate/something/1.2.3")]
    #[test_case("/something/latest/some_path/", "/crate/something/latest")]
    fn rustdoc_page_from_failed_build_redirects_to_crate(path: &str, expected: &str) {
        wrapper(|env| {
            env.fake_release()
                .name("something")
                .version("1.2.3")
                .archive_storage(true)
                .build_result_failed()
                .create()?;
            let web = env.frontend();

            assert_redirect_cached(
                path,
                expected,
                CachePolicy::ForeverInCdn,
                web,
                &env.config(),
            )?;

            Ok(())
        })
    }
}
