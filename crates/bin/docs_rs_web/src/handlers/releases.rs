//! Releases web handlersrelease

use crate::{
    Config,
    cache::CachePolicy,
    error::{AxumNope, AxumResult},
    extractors::{DbConnection, Path, rustdoc::RustdocParams},
    handlers::{axum_redirect, rustdoc::OfficialCrateDescription},
    impl_axum_webpage,
    match_release::match_version,
    metrics::WebMetrics,
    page::templates::{RenderBrands, RenderRegular, RenderSolid, filters},
};
use anyhow::{Context as _, Result, anyhow};
use askama::Template;
use axum::{
    extract::{Extension, Query},
    response::{IntoResponse, Response as AxumResponse},
};
use base64::{Engine, engine::general_purpose::STANDARD as b64};
use chrono::{DateTime, Utc};
use docs_rs_build_queue::{AsyncBuildQueue, PRIORITY_CONTINUOUS, QueuedCrate};
use docs_rs_registry_api::{self as registry_api, RegistryApi};
use docs_rs_types::{KrateName, ReqVersion, Version};
use docs_rs_uri::encode_url_path;
use futures_util::stream::TryStreamExt;
use http::StatusCode;
use serde::Deserialize;
use sqlx::Row;
use std::{
    collections::{BTreeMap, HashMap},
    str,
    sync::Arc,
};
use tracing::{error, trace, warn};
use url::form_urlencoded;

/// Number of release in home page
const RELEASES_IN_HOME: i64 = 15;
/// Releases in /releases page
const RELEASES_IN_RELEASES: i64 = 30;
/// Releases in recent releases feed
const RELEASES_IN_FEED: i64 = 150;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Release {
    pub(crate) name: KrateName,
    pub(crate) version: Version,
    pub(crate) description: Option<String>,
    pub(crate) target_name: Option<String>,
    pub(crate) rustdoc_status: bool,
    pub(crate) build_time: Option<DateTime<Utc>>,
    pub(crate) stars: i32,
    pub(crate) has_unyanked_releases: Option<bool>,
}

impl Release {
    pub fn rustdoc_params(&self) -> RustdocParams {
        RustdocParams::new(self.name.clone())
            .with_req_version(self.version.clone())
            .with_maybe_target_name(self.target_name.clone())
    }
}

#[derive(Debug, Copy, Clone, PartialEq, Eq, Default)]
pub(crate) enum Order {
    #[default]
    ReleaseTime,
    GithubStars,
    RecentFailures,
    FailuresByGithubStars,
}

pub(crate) async fn get_releases(
    conn: &mut sqlx::PgConnection,
    page: i64,
    limit: i64,
    order: Order,
    latest_only: bool,
) -> Result<Vec<Release>> {
    let offset = (page - 1) * limit;

    // WARNING: it is _crucial_ that this always be hard-coded and NEVER be user input
    let (ordering, filter_failed): (&'static str, _) = match order {
        Order::ReleaseTime => ("release_build_status.last_build_time", false),
        Order::GithubStars => ("repositories.stars", false),
        Order::RecentFailures => ("release_build_status.last_build_time", true),
        Order::FailuresByGithubStars => ("repositories.stars", true),
    };

    let query = format!(
        r#"SELECT crates.name,
            releases.version as "version: Version",
            releases.description,
            releases.target_name,
            releases.rustdoc_status,
            release_build_status.last_build_time,
            repositories.stars
        FROM crates
        {1}
        INNER JOIN release_build_status ON releases.id = release_build_status.rid
        LEFT JOIN repositories ON releases.repository_id = repositories.id
        WHERE
            ((NOT $3) OR (release_build_status.build_status = 'failure' AND releases.is_library = TRUE))
            AND {0} IS NOT NULL AND
            release_build_status.build_status != 'in_progress'

        ORDER BY {0} DESC
        LIMIT $1 OFFSET $2"#,
        ordering,
        if latest_only {
            "INNER JOIN releases ON crates.latest_version_id = releases.id"
        } else {
            "INNER JOIN releases ON crates.id = releases.crate_id"
        }
    );

    Ok(sqlx::query(query.as_str())
        .bind(limit)
        .bind(offset)
        .bind(filter_failed)
        .fetch(conn)
        .map_ok(|row| Release {
            name: row.get(0),
            version: row.get(1),
            description: row.get(2),
            target_name: row.get(3),
            rustdoc_status: row.get::<Option<bool>, _>(4).unwrap_or(false),
            build_time: row.get(5),
            stars: row.get::<Option<i32>, _>(6).unwrap_or(0),
            has_unyanked_releases: None,
        })
        .try_collect()
        .await?)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ReleaseStatus {
    Available(Release),
    External(&'static OfficialCrateDescription),
    /// Only contains the crate name.
    NotAvailable(KrateName),
}

struct SearchResult {
    pub results: Vec<ReleaseStatus>,
    pub prev_page: Option<String>,
    pub next_page: Option<String>,
}

/// Get the search results for a crate search query
///
/// This delegates to the crates.io search API.
async fn get_search_results(
    conn: &mut sqlx::PgConnection,
    registry: &RegistryApi,
    query_params: &str,
    query: &str,
) -> Result<SearchResult, registry_api::Error> {
    let registry_api::Search { crates, meta } = registry.search(query_params).await?;

    let names = Arc::new(
        crates
            .into_iter()
            .map(|krate| {
                krate
                    .name
                    .parse::<KrateName>()
                    .expect("results from crates.io always pass the check")
            })
            .collect::<Vec<_>>(),
    );

    // now we're trying to get the docs.rs data for the crates
    // returned by the search.
    // Docs.rs might not know about crates or `max_version` shortly after
    // they were published on crates.io, or while the build is running.
    // So for now we are using the version with the youngest release_time.
    // This is different from all other release-list views where we show
    // our latest build.
    let mut crates: HashMap<KrateName, Release> = sqlx::query!(
        r#"SELECT
               crates.name as "name: KrateName",
               releases.version as "version: Version",
               releases.description,
               release_build_status.last_build_time,
               releases.target_name,
               releases.rustdoc_status,
               repositories.stars as "stars?",
               EXISTS (
                   SELECT 1
                   FROM releases AS all_releases
                   WHERE
                       all_releases.crate_id = crates.id AND
                       all_releases.yanked = false
               ) AS has_unyanked_releases

           FROM crates
           INNER JOIN releases ON crates.latest_version_id = releases.id
           INNER JOIN release_build_status ON releases.id = release_build_status.rid
           LEFT JOIN repositories ON releases.repository_id = repositories.id

           WHERE
               crates.name = ANY($1) AND
               release_build_status.build_status <> 'in_progress'"#,
        &names[..] as _,
    )
    .fetch(&mut *conn)
    .map_ok(|row| {
        (
            row.name.clone(),
            Release {
                name: row.name,
                version: row.version,
                description: row.description,
                build_time: row.last_build_time,
                target_name: row.target_name,
                rustdoc_status: row.rustdoc_status.unwrap_or(false),
                stars: row.stars.unwrap_or(0),
                has_unyanked_releases: row.has_unyanked_releases,
            },
        )
    })
    .try_collect()
    .await
    .map_err(|err| anyhow!(err))?;

    // start with the original names from crates.io to keep the original ranking,
    // extend with the release/build information from docs.rs
    // Crates that are not on docs.rs yet will not be returned.
    let mut results = Vec::new();
    if let Ok(krate) = query.parse::<KrateName>()
        && let Some(desc) = super::rustdoc::DOC_RUST_LANG_ORG_REDIRECTS.get(&krate)
    {
        results.push(ReleaseStatus::External(desc));
    }

    let names: Vec<KrateName> =
        Arc::into_inner(names).expect("Arc still borrowed in `get_search_results`");
    results.extend(names.into_iter().map(|name| {
        if let Some(release) = crates.remove(&name) {
            ReleaseStatus::Available(release)
        } else {
            ReleaseStatus::NotAvailable(name)
        }
    }));

    Ok(SearchResult {
        results,
        prev_page: meta.prev_page,
        next_page: meta.next_page,
    })
}

#[derive(Template)]
#[template(path = "core/home.html")]
#[derive(Debug, Clone, PartialEq, Eq)]
struct HomePage {
    recent_releases: Vec<Release>,
}

impl_axum_webpage! {
    HomePage,
    cache_policy = |_| CachePolicy::ShortInCdnAndBrowser,
}

pub(crate) async fn home_page(mut conn: DbConnection) -> AxumResult<impl IntoResponse> {
    let recent_releases =
        get_releases(&mut conn, 1, RELEASES_IN_HOME, Order::ReleaseTime, true).await?;

    Ok(HomePage { recent_releases })
}

#[derive(Template)]
#[template(path = "releases/feed.xml")]
#[derive(Debug, Clone, PartialEq, Eq)]
struct ReleaseFeed {
    recent_releases: Vec<Release>,
}

impl_axum_webpage! {
    ReleaseFeed,
    content_type = "application/xml",
}

pub(crate) async fn releases_feed_handler(mut conn: DbConnection) -> AxumResult<impl IntoResponse> {
    let recent_releases =
        get_releases(&mut conn, 1, RELEASES_IN_FEED, Order::ReleaseTime, true).await?;
    Ok(ReleaseFeed { recent_releases })
}

#[derive(Template)]
#[template(path = "releases/releases.html")]
#[derive(Debug, Clone, PartialEq, Eq)]
struct ViewReleases {
    releases: Vec<ReleaseStatus>,
    description: String,
    release_type: ReleaseType,
    show_next_page: bool,
    show_previous_page: bool,
    page_number: i64,
    owner: Option<String>,
}

impl_axum_webpage! { ViewReleases }

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub(crate) enum ReleaseType {
    Recent,
    Stars,
    RecentFailures,
    Failures,
    Search,
}

impl PartialEq<&str> for ReleaseType {
    fn eq(&self, other: &&str) -> bool {
        self.as_str() == *other
    }
}
impl PartialEq<str> for ReleaseType {
    fn eq(&self, other: &str) -> bool {
        self.as_str() == other
    }
}

impl ReleaseType {
    fn as_str(&self) -> &str {
        match self {
            Self::Recent => "recent",
            Self::Stars => "stars",
            Self::RecentFailures => "recent-failures",
            Self::Failures => "failures",
            Self::Search => "search",
        }
    }
}

pub(crate) async fn releases_handler(
    conn: &mut sqlx::PgConnection,
    page: Option<i64>,
    release_type: ReleaseType,
) -> AxumResult<impl IntoResponse + use<>> {
    let page_number = page.unwrap_or(1);

    let (description, release_order, latest_only) = match release_type {
        ReleaseType::Recent => ("Recently uploaded crates", Order::ReleaseTime, false),
        ReleaseType::Stars => ("Crates with most stars", Order::GithubStars, true),
        ReleaseType::RecentFailures => (
            "Recent crates failed to build",
            Order::RecentFailures,
            false,
        ),
        ReleaseType::Failures => (
            "Crates with most stars failed to build",
            Order::FailuresByGithubStars,
            true,
        ),

        ReleaseType::Search => {
            panic!("The search page has special requirements and cannot use this handler",)
        }
    };

    let releases = get_releases(
        &mut *conn,
        page_number,
        RELEASES_IN_RELEASES,
        release_order,
        latest_only,
    )
    .await?;

    // Show next and previous page buttons
    let (show_next_page, show_previous_page) = (
        releases.len() == RELEASES_IN_RELEASES as usize,
        page_number != 1,
    );

    Ok(ViewReleases {
        releases: releases
            .into_iter()
            .map(ReleaseStatus::Available)
            .collect::<Vec<_>>(),
        description: description.into(),
        release_type,
        show_next_page,
        show_previous_page,
        page_number,
        owner: None,
    })
}

pub(crate) async fn recent_releases_handler(
    page: Option<Path<i64>>,
    mut conn: DbConnection,
) -> AxumResult<impl IntoResponse> {
    releases_handler(&mut conn, page.map(|p| p.0), ReleaseType::Recent).await
}

pub(crate) async fn releases_by_stars_handler(
    page: Option<Path<i64>>,
    mut conn: DbConnection,
) -> AxumResult<impl IntoResponse> {
    releases_handler(&mut conn, page.map(|p| p.0), ReleaseType::Stars).await
}

pub(crate) async fn releases_recent_failures_handler(
    page: Option<Path<i64>>,
    mut conn: DbConnection,
) -> AxumResult<impl IntoResponse> {
    releases_handler(&mut conn, page.map(|p| p.0), ReleaseType::RecentFailures).await
}

pub(crate) async fn releases_failures_by_stars_handler(
    page: Option<Path<i64>>,
    mut conn: DbConnection,
) -> AxumResult<impl IntoResponse> {
    releases_handler(&mut conn, page.map(|p| p.0), ReleaseType::Failures).await
}

pub(crate) async fn owner_handler(Path(owner): Path<String>) -> AxumResult<impl IntoResponse> {
    axum_redirect(format!(
        "https://crates.io/users/{}",
        encode_url_path(owner.strip_prefix('@').unwrap_or(&owner))
    ))
    .map_err(|_| AxumNope::OwnerNotFound)
}

#[derive(Template)]
#[template(path = "releases/search_results.html")]
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Search {
    pub(crate) title: String,
    pub(crate) message: Option<String>,
    pub(crate) releases: Vec<ReleaseStatus>,
    pub(crate) search_query: Option<String>,
    pub(crate) search_sort_by: Option<String>,
    pub(crate) previous_page_link: Option<String>,
    pub(crate) next_page_link: Option<String>,
    /// This should always be `ReleaseType::Search`
    pub(crate) release_type: ReleaseType,
    pub(crate) status: http::StatusCode,
}

impl Default for Search {
    fn default() -> Self {
        Self {
            title: String::default(),
            message: None,
            releases: Vec::default(),
            search_query: None,
            previous_page_link: None,
            next_page_link: None,
            search_sort_by: None,
            release_type: ReleaseType::Search,
            status: http::StatusCode::OK,
        }
    }
}

async fn redirect_to_random_crate(
    config: Arc<Config>,
    otel_metrics: Arc<WebMetrics>,
    conn: &mut sqlx::PgConnection,
) -> AxumResult<impl IntoResponse + use<>> {
    // We try to find a random crate and redirect to it.
    //
    // The query is efficient, but relies on a static factor which depends
    // on the amount of crates with > 100 GH stars over the amount of all crates.
    //
    // If random-crate-searches end up being empty, increase that value.
    let row = sqlx::query!(
        r#"WITH params AS (
            -- get maximum possible id-value in crates-table
            SELECT last_value AS max_id FROM crates_id_seq
        )
        SELECT
            crates.name as "name: KrateName",
            releases.version as "version: Version",
            releases.target_name
        FROM (
            -- generate random numbers in the ID-range.
            SELECT DISTINCT 1 + trunc(random() * params.max_id)::INTEGER AS id
            FROM params, generate_series(1, $1)
        ) AS r
        INNER JOIN crates ON r.id = crates.id
        INNER JOIN releases ON crates.latest_version_id = releases.id
        INNER JOIN repositories ON releases.repository_id = repositories.id
        WHERE
            releases.rustdoc_status = TRUE AND
            repositories.stars >= 100
        LIMIT 1"#,
        config.random_crate_search_view_size as i32,
    )
    .fetch_optional(&mut *conn)
    .await
    .context("error fetching random crate")?;

    if let Some(row) = row {
        otel_metrics.im_feeling_lucky_searches.add(1, &[]);

        let params = RustdocParams::new(row.name.clone())
            .with_req_version(ReqVersion::Exact(row.version.clone()))
            .with_maybe_target_name(row.target_name.as_deref());

        trace!(?row, ?params, "redirecting to random crate result");

        Ok(axum_redirect(params.rustdoc_url())?)
    } else {
        error!("found no result in random crate search");
        Err(AxumNope::NoResults)
    }
}

impl_axum_webpage! {
    Search,
    status = |search| search.status,
}

pub(crate) async fn search_handler(
    mut conn: DbConnection,
    Extension(config): Extension<Arc<Config>>,
    Extension(registry): Extension<Arc<RegistryApi>>,
    Extension(otel_metrics): Extension<Arc<WebMetrics>>,
    Query(mut query_params): Query<HashMap<String, String>>,
) -> AxumResult<AxumResponse> {
    let mut query = query_params
        .get("query")
        .map(|q| q.to_string())
        .unwrap_or_else(|| "".to_string());
    let mut sort_by = query_params
        .get("sort")
        .map(|q| q.to_string())
        .unwrap_or_else(|| "relevance".to_string());
    // check if I am feeling lucky button pressed and redirect user to crate page
    // if there is a match. Also check for paths to items within crates.
    if query_params.remove("i-am-feeling-lucky").is_some() || query.contains("::") {
        // redirect to a random crate if query is empty
        if query.is_empty() {
            return Ok(redirect_to_random_crate(config, otel_metrics, &mut conn)
                .await?
                .into_response());
        }

        let mut queries = BTreeMap::new();

        let krate = match query.split_once("::") {
            Some((krate, query)) => {
                queries.insert("search".into(), query.into());
                krate
            }
            None => &query,
        };

        // since we never pass a version into `match_version` here, we'll never get
        // `MatchVersion::Exact`, so the distinction between `Exact` and `Semver` doesn't
        // matter
        if let Ok(krate) = krate.parse::<KrateName>()
            && let Ok(matchver) = match_version(&mut conn, &krate, &ReqVersion::Latest)
                .await
                .map(|matched_release| matched_release.into_exactly_named())
        {
            query_params.remove("query");
            queries.extend(query_params);

            let rustdoc_status = matchver.rustdoc_status();
            let params = RustdocParams::from_matched_release(&matchver);

            trace!(
                %krate,
                ?params,
                "redirecting I'm feeling lucky search to crate page"
            );

            let uri = if rustdoc_status {
                params.rustdoc_url().append_query_pairs(queries)
            } else {
                params.crate_details_url()
            };

            return Ok(super::axum_redirect(uri)?.into_response());
        }
    }

    let search_result = if let Some(paginate) = query_params.get("paginate") {
        let decoded = b64.decode(paginate.as_bytes()).map_err(|e| {
            warn!("error when decoding pagination base64 string \"{paginate}\": {e:?}");
            AxumNope::NoResults
        })?;
        let query_params = String::from_utf8_lossy(&decoded);
        let query_params = query_params.strip_prefix('?').ok_or_else(|| {
            // sometimes we see plain bytes being passed to `paginate`.
            // In these cases we just return `NoResults` and don't call
            // the crates.io API.
            // The whole point of the `paginate` design is that we don't
            // know anything about the pagination args and crates.io can
            // change them as they wish, so we cannot do any more checks here.
            warn!("didn't get query args in `paginate` arguments for search: \"{query_params}\"");
            AxumNope::NoResults
        })?;

        for (k, v) in form_urlencoded::parse(query_params.as_bytes()) {
            match &*k {
                "q" => query = v.to_string(),
                "sort" => sort_by = v.to_string(),
                _ => {}
            }
        }

        get_search_results(&mut conn, &registry, query_params, "").await
    } else if !query.is_empty() {
        let query_params: String = form_urlencoded::Serializer::new(String::new())
            .append_pair("q", &query)
            .append_pair("sort", &sort_by)
            .append_pair("per_page", &RELEASES_IN_RELEASES.to_string())
            .finish();

        get_search_results(&mut conn, &registry, &query_params, &query).await
    } else {
        return Err(AxumNope::NoResults);
    };

    match search_result {
        Ok(search_result) => {
            let title = if search_result.results.is_empty() {
                format!("No results found for '{query}'")
            } else {
                format!("Search results for '{query}'")
            };

            Ok(Search {
                title,
                releases: search_result.results,
                search_query: Some(query),
                search_sort_by: Some(sort_by),
                next_page_link: search_result
                    .next_page
                    .map(|params| format!("/releases/search?paginate={}", b64.encode(params))),
                previous_page_link: search_result
                    .prev_page
                    .map(|params| format!("/releases/search?paginate={}", b64.encode(params))),
                ..Default::default()
            }
            .into_response())
        }
        Err(err) => {
            warn!(%query, ?err, "error during crate search");

            Ok(Search {
                title: format!("Error searching for '{query}'"),
                message: Some(err.to_string()),
                status: err.status().unwrap_or(StatusCode::INTERNAL_SERVER_ERROR),
                ..Default::default()
            }
            .into_response())
        }
    }
}

#[derive(Template)]
#[template(path = "releases/activity.html")]
#[derive(Debug, Clone, PartialEq)]
struct ReleaseActivity {
    description: &'static str,
    dates: Vec<String>,
    counts: Vec<i64>,
    failures: Vec<i64>,
}

impl_axum_webpage! { ReleaseActivity }

pub(crate) async fn activity_handler(mut conn: DbConnection) -> AxumResult<impl IntoResponse> {
    let rows: Vec<_> = sqlx::query!(
        r#"WITH dates AS (
               -- we need this series so that days in the statistic that don't have any releases are included
               SELECT generate_series(
                       CURRENT_DATE - INTERVAL '30 days',
                       CURRENT_DATE - INTERVAL '1 day',
                       '1 day'::interval
                   )::date AS date_
           ),
           release_stats AS (
               SELECT
                   release_time::date AS date_,
                   SUM(CAST(
                       release_build_status.build_status != 'in_progress' AS INT
                   )) AS counts,
                   SUM(CAST((
                       is_library = TRUE AND
                       release_build_status.build_status = 'failure'
                   ) AS INT)) AS failures
               FROM releases
               INNER JOIN release_build_status ON releases.id = release_build_status.rid

               WHERE
                   release_time >= CURRENT_DATE - INTERVAL '30 days' AND
                   release_time < CURRENT_DATE
               GROUP BY
                   release_time::date
           )
           SELECT
               dates.date_ AS "date!",
               COALESCE(rs.counts, 0) AS "counts!",
               COALESCE(rs.failures, 0) AS "failures!"
           FROM
               dates
               LEFT OUTER JOIN Release_stats AS rs ON dates.date_ = rs.date_

               ORDER BY
                   dates.date_
        "#)
        .fetch(&mut *conn)
        .try_collect().await.context("error fetching data")?;

    Ok(ReleaseActivity {
        description: "Monthly release activity",
        dates: rows
            .iter()
            .map(|row| row.date.format("%d %b").to_string())
            .collect(),
        counts: rows.iter().map(|rows| rows.counts).collect(),
        failures: rows.iter().map(|rows| rows.failures).collect(),
    })
}

#[derive(Template)]
#[template(path = "releases/build_queue.html")]
#[derive(Debug, Clone, PartialEq)]
struct BuildQueuePage {
    description: &'static str,
    queue: Vec<QueuedCrate>,
    rebuild_queue: Vec<QueuedCrate>,
    in_progress_builds: Vec<(KrateName, Version)>,
    expand_rebuild_queue: bool,
}

impl_axum_webpage! { BuildQueuePage }

#[derive(Deserialize)]
pub(crate) struct BuildQueueParams {
    expand: Option<String>,
}

pub(crate) async fn build_queue_handler(
    Extension(build_queue): Extension<Arc<AsyncBuildQueue>>,
    mut conn: DbConnection,
    Query(params): Query<BuildQueueParams>,
) -> AxumResult<impl IntoResponse> {
    let in_progress_builds: Vec<(KrateName, Version)> = sqlx::query!(
        r#"SELECT
            crates.name as "name: KrateName",
            releases.version as "version: Version"
         FROM builds
         INNER JOIN releases ON releases.id = builds.rid
         INNER JOIN crates ON releases.crate_id = crates.id
         WHERE
            builds.build_status = 'in_progress'
         ORDER BY builds.id ASC"#
    )
    .fetch_all(&mut *conn)
    .await?
    .into_iter()
    .map(|rec| (rec.name, rec.version))
    .collect();

    let mut rebuild_queue = Vec::new();
    let mut queue = build_queue
        .queued_crates()
        .await?
        .into_iter()
        .filter(|krate| {
            !in_progress_builds.iter().any(|(name, version)| {
                // use `.any` instead of `.contains` to avoid cloning name& version for the match
                *name == krate.name && *version == krate.version
            })
        })
        .collect::<Vec<_>>();

    queue.retain_mut(|krate| {
        if krate.priority >= PRIORITY_CONTINUOUS {
            rebuild_queue.push(krate.clone());
            false
        } else {
            // The priority here is inverted: in the database if a crate has a higher priority it
            // will be built after everything else, which is counter-intuitive for people not
            // familiar with docs.rs's inner workings.
            krate.priority = -krate.priority;
            true
        }
    });

    Ok(BuildQueuePage {
        description: "crate documentation scheduled to build & deploy",
        queue,
        rebuild_queue,
        in_progress_builds,
        expand_rebuild_queue: params.expand.is_some(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{
        AxumResponseTestExt, AxumRouterTestExt, TestEnvironment, TestEnvironmentExt as _,
        async_wrapper,
    };
    use anyhow::Error;
    use chrono::{Duration, TimeZone};
    use docs_rs_database::releases::{
        finish_build, initialize_build, initialize_crate, initialize_release,
    };
    use docs_rs_registry_api::{CrateOwner, OwnerKind};
    use docs_rs_test_fakes::{FakeBuild, fake_release_that_failed_before_build};
    use docs_rs_types::{
        BuildStatus,
        testing::{BAR, BAZ, FOO, V0_1, V1, V2, V3},
    };
    use kuchikiki::traits::TendrilSink;
    use mockito::Matcher;
    use reqwest::StatusCode;
    use serde_json::json;
    use std::collections::HashSet;
    use test_case::test_case;

    #[test]
    fn test_release_list_with_incomplete_release_and_successful_build() {
        async_wrapper(|env| async move {
            let mut conn = env.async_conn().await?;

            let crate_id = initialize_crate(&mut conn, &FOO).await?;
            let release_id = initialize_release(&mut conn, crate_id, &V1).await?;
            let build_id = initialize_build(&mut conn, release_id).await?;

            finish_build(
                &mut conn,
                build_id,
                "rustc-version",
                "docs.rs 4.0.0",
                BuildStatus::Success,
                None,
                None,
            )
            .await?;

            let releases = get_releases(&mut conn, 1, 10, Order::ReleaseTime, false).await?;

            assert_eq!(
                vec!["foo"],
                releases
                    .iter()
                    .map(|release| release.name.to_string())
                    .collect::<Vec<_>>(),
            );

            Ok(())
        })
    }

    #[test]
    fn get_releases_by_stars() {
        async_wrapper(|env| async move {
            let db = env.pool()?;

            env.fake_release()
                .await
                .name("foo")
                .version(V1)
                .github_stats("ghost/foo", 10, 10, 10)
                .create()
                .await?;
            env.fake_release()
                .await
                .name("bar")
                .version(V1)
                .github_stats("ghost/bar", 20, 20, 20)
                .create()
                .await?;
            env.fake_release()
                .await
                .name("bar")
                .version(V1)
                .github_stats("ghost/bar", 20, 20, 20)
                .create()
                .await?;
            // release without stars will not be shown
            env.fake_release()
                .await
                .name("baz")
                .version(V1)
                .create()
                .await?;

            // release with only in-progress build (= in progress release) will not be shown
            env.fake_release()
                .await
                .name("in_progress")
                .version(V0_1)
                .builds(vec![
                    FakeBuild::default()
                        .build_status(BuildStatus::InProgress)
                        .rustc_version("rustc (blabla 2022-01-01)")
                        .docsrs_version("docs.rs 4.0.0"),
                ])
                .create()
                .await?;

            let releases =
                get_releases(&mut *db.get_async().await?, 1, 10, Order::GithubStars, true)
                    .await
                    .unwrap();
            assert_eq!(
                vec![
                    "bar", // 20 stars
                    "foo", // 10 stars
                ],
                releases
                    .iter()
                    .map(|release| release.name.to_string())
                    .collect::<Vec<_>>(),
            );

            Ok(())
        })
    }

    #[test]
    fn search_im_feeling_lucky_with_query_redirect_to_crate_page() {
        async_wrapper(|env| async move {
            let web = env.web_app().await;
            env.fake_release()
                .await
                .name("some_random_crate")
                .version(V1)
                .build_result_failed()
                .create()
                .await?;
            env.fake_release()
                .await
                .name("some_other_crate")
                .version(V1)
                .create()
                .await?;

            web.assert_redirect(
                "/releases/search?query=some_random_crate&i-am-feeling-lucky=1",
                "/crate/some_random_crate/latest",
            )
            .await?;
            Ok(())
        })
    }

    #[test]
    fn search_im_feeling_lucky_with_query_redirect_to_docs() {
        async_wrapper(|env| async move {
            let web = env.web_app().await;
            env.fake_release()
                .await
                .name("some_random_crate")
                .version(V1)
                .create()
                .await?;
            env.fake_release()
                .await
                .name("some_other_crate")
                .version(V1)
                .create()
                .await?;

            web.assert_redirect(
                "/releases/search?query=some_random_crate&i-am-feeling-lucky=1",
                "/some_random_crate/latest/some_random_crate/",
            )
            .await?;
            Ok(())
        })
    }

    #[test]
    fn im_feeling_lucky_with_stars() {
        async_wrapper(|env| async move {
            // The normal test-setup will offset all primary sequences by 10k
            // to prevent errors with foreign key relations.
            // Random-crate-search relies on the sequence for the crates-table
            // to find a maximum possible ID. This combined with only one actual
            // crate in the db breaks this test.
            // That's why we reset the id-sequence to zero for this test.

            let mut conn = env.async_conn().await?;
            sqlx::query!(r#"ALTER SEQUENCE crates_id_seq RESTART WITH 1"#)
                .execute(&mut *conn)
                .await?;

            let web = env.web_app().await;
            env.fake_release()
                .await
                .github_stats("some/repo", 333, 22, 11)
                .name("some_random_crate")
                .version(V1)
                .create()
                .await?;
            web.assert_redirect(
                "/releases/search?query=&i-am-feeling-lucky=1",
                &format!("/some_random_crate/{V1}/some_random_crate/"),
            )
            .await?;
            Ok(())
        })
    }

    #[test]
    fn search_coloncolon_path_redirects_to_crate_docs() {
        async_wrapper(|env| async move {
            let web = env.web_app().await;
            env.fake_release()
                .await
                .name("some_random_crate")
                .create()
                .await?;
            env.fake_release()
                .await
                .name("some_other_crate")
                .create()
                .await?;

            web.assert_redirect(
                "/releases/search?query=some_random_crate::somepath",
                "/some_random_crate/latest/some_random_crate/?search=somepath",
            )
            .await?;
            web.assert_redirect(
                "/releases/search?query=some_random_crate::some::path",
                "/some_random_crate/latest/some_random_crate/?search=some%3A%3Apath",
            )
            .await?;
            Ok(())
        })
    }

    #[test]
    fn search_coloncolon_path_redirects_to_crate_docs_and_keeps_query() {
        async_wrapper(|env| async move {
            let web = env.web_app().await;
            env.fake_release()
                .await
                .name("some_random_crate")
                .create()
                .await?;

            web.assert_redirect(
                "/releases/search?query=some_random_crate::somepath&go_to_first=true",
                "/some_random_crate/latest/some_random_crate/?go_to_first=true&search=somepath",
            )
            .await?;
            Ok(())
        })
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn search_result_can_retrieve_sort_by_from_pagination() -> Result<()> {
        let mut crates_io = mockito::Server::new_async().await;

        let env = TestEnvironment::builder()
            .registry_api_config(
                docs_rs_registry_api::Config::builder()
                    .registry_api_host(crates_io.url().parse().unwrap())
                    .build(),
            )
            .build()
            .await?;

        let web = env.web_app().await;
        env.fake_release()
            .await
            .name("some_random_crate")
            .create()
            .await?;

        let _m = crates_io
            .mock("GET", "/api/v1/crates")
            .match_query(Matcher::AllOf(vec![
                Matcher::UrlEncoded("q".into(), "some_random_crate".into()),
                Matcher::UrlEncoded("per_page".into(), "30".into()),
                Matcher::UrlEncoded("page".into(), "2".into()),
                Matcher::UrlEncoded("sort".into(), "recent-updates".into()),
            ]))
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(
                json!({
                    "crates": [
                        { "name": "some_random_crate" },
                    ],
                    "meta": {
                        "next_page": "?q=some_random_crate&sort=recent-updates&per_page=30&page=2",
                        "prev_page": "?q=some_random_crate&sort=recent-updates&per_page=30&page=1",
                    }
                })
                .to_string(),
            )
            .create_async()
            .await;

        // click the "Next Page" Button, the "Sort by" SelectBox should keep the same option.
        let next_page_url = format!(
            "/releases/search?paginate={}",
            b64.encode("?q=some_random_crate&sort=recent-updates&per_page=30&page=2"),
        );
        let response = web.get(&next_page_url).await?;
        assert!(response.status().is_success());

        let page = kuchikiki::parse_html().one(response.text().await?);
        let is_target_option_selected = page
            .select("#nav-sort > option")
            .expect("missing option")
            .any(|el| {
                let attributes = el.attributes.borrow();
                attributes.get("selected").is_some()
                    && attributes.get("value").unwrap() == "recent-updates"
            });
        assert!(is_target_option_selected);

        Ok(())
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn search_result_passes_cratesio_pagination_links() -> Result<()> {
        let mut crates_io = mockito::Server::new_async().await;

        let env = TestEnvironment::builder()
            .registry_api_config(
                docs_rs_registry_api::Config::builder()
                    .registry_api_host(crates_io.url().parse().unwrap())
                    .build(),
            )
            .build()
            .await?;

        let web = env.web_app().await;
        env.fake_release()
            .await
            .name("some_random_crate")
            .create()
            .await?;

        let _m = crates_io
            .mock("GET", "/api/v1/crates")
            .match_query(Matcher::AllOf(vec![
                Matcher::UrlEncoded("q".into(), "some_random_crate".into()),
                Matcher::UrlEncoded("per_page".into(), "30".into()),
            ]))
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(
                json!({
                    "crates": [
                        { "name": "some_random_crate" },
                    ],
                    "meta": {
                        "next_page": "?some=parameters&that=cratesio&might=return",
                        "prev_page": "?and=the&parameters=for&the=previouspage",
                    }
                })
                .to_string(),
            )
            .create_async()
            .await;

        let response = web.get("/releases/search?query=some_random_crate").await?;
        assert!(response.status().is_success());

        let page = kuchikiki::parse_html().one(response.text().await?);

        let other_search_links: Vec<_> = page
            .select("a")
            .expect("missing link")
            .map(|el| {
                let attributes = el.attributes.borrow();
                attributes.get("href").unwrap().to_string()
            })
            .filter(|url| url.starts_with("/releases/search?"))
            .collect();

        assert_eq!(other_search_links.len(), 2);
        assert_eq!(
            other_search_links[0],
            format!(
                "/releases/search?paginate={}",
                b64.encode("?and=the&parameters=for&the=previouspage"),
            )
        );
        assert_eq!(
            other_search_links[1],
            format!(
                "/releases/search?paginate={}",
                b64.encode("?some=parameters&that=cratesio&might=return")
            )
        );

        Ok(())
    }

    #[test]
    fn search_invalid_paginate_doesnt_request_cratesio() {
        async_wrapper(|env| async move {
            let response = env
                .web_app()
                .await
                .get(&format!(
                    "/releases/search?paginate={}",
                    b64.encode("something_that_doesnt_start_with_?")
                ))
                .await?;
            assert_eq!(response.status(), StatusCode::NOT_FOUND);
            Ok(())
        })
    }

    #[test_case(StatusCode::NOT_FOUND)]
    #[test_case(StatusCode::INTERNAL_SERVER_ERROR)]
    #[test_case(StatusCode::BAD_GATEWAY)]
    #[tokio::test(flavor = "multi_thread")]
    async fn crates_io_errors_are_correctly_returned_and_we_dont_try_parsing(
        status: StatusCode,
    ) -> Result<()> {
        let mut crates_io = mockito::Server::new_async().await;

        let env = TestEnvironment::builder()
            .registry_api_config(
                docs_rs_registry_api::Config::builder()
                    .registry_api_host(crates_io.url().parse().unwrap())
                    .crates_io_api_call_retries(0)
                    .build(),
            )
            .build()
            .await?;

        let _m = crates_io
            .mock("GET", "/api/v1/crates")
            .match_query(Matcher::AllOf(vec![
                Matcher::UrlEncoded("q".into(), "doesnt_matter_here".into()),
                Matcher::UrlEncoded("per_page".into(), "30".into()),
            ]))
            .with_status(status.as_u16() as usize)
            .create_async()
            .await;

        let response = env
            .web_app()
            .await
            .get("/releases/search?query=doesnt_matter_here")
            .await?;
        assert_eq!(response.status(), status);

        assert!(response.text().await?.contains(&format!("{status}")));
        Ok(())
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn search_encoded_pagination_passed_to_cratesio() -> Result<()> {
        let mut crates_io = mockito::Server::new_async().await;

        let env = TestEnvironment::builder()
            .registry_api_config(
                docs_rs_registry_api::Config::builder()
                    .registry_api_host(crates_io.url().parse().unwrap())
                    .build(),
            )
            .build()
            .await?;

        let web = env.web_app().await;
        env.fake_release()
            .await
            .name("some_random_crate")
            .create()
            .await?;

        let _m = crates_io
            .mock("GET", "/api/v1/crates")
            .match_query(Matcher::AllOf(vec![
                Matcher::UrlEncoded("some".into(), "dummy".into()),
                Matcher::UrlEncoded("pagination".into(), "parameters".into()),
            ]))
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(
                json!({
                    "crates": [
                        { "name": "some_random_crate" },
                    ],
                    "meta": {
                        "next_page": null,
                        "prev_page": null,
                    }
                })
                .to_string(),
            )
            .create_async()
            .await;

        let links = get_release_links(
            &format!(
                "/releases/search?paginate={}",
                b64.encode("?some=dummy&pagination=parameters")
            ),
            &web,
        )
        .await?;

        assert_eq!(links.len(), 1);
        assert_eq!(links[0], "/some_random_crate/latest/some_random_crate/",);
        Ok(())
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn search_lucky_with_unknown_crate() -> Result<()> {
        let mut crates_io = mockito::Server::new_async().await;

        let env = TestEnvironment::builder()
            .registry_api_config(
                docs_rs_registry_api::Config::builder()
                    .registry_api_host(crates_io.url().parse().unwrap())
                    .build(),
            )
            .build()
            .await?;

        let web = env.web_app().await;
        env.fake_release()
            .await
            .name("some_random_crate")
            .create()
            .await?;

        let _m = crates_io
            .mock("GET", "/api/v1/crates")
            .match_query(Matcher::AllOf(vec![
                Matcher::UrlEncoded("q".into(), "some_random_".into()),
                Matcher::UrlEncoded("per_page".into(), "30".into()),
            ]))
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(
                json!({
                    "crates": [
                        { "name": "some_random_crate" },
                        { "name": "some_other_crate" },
                    ],
                    "meta": {
                        "next_page": null,
                        "prev_page": null,
                    }
                })
                .to_string(),
            )
            .create_async()
            .await;

        // when clicking "I'm feeling lucky" and the query doesn't match any crate,
        // just fallback to the normal search results.
        let links = get_release_links(
            "/releases/search?query=some_random_&i-am-feeling-lucky=1",
            &web,
        )
        .await?;

        assert_eq!(links.len(), 1);
        assert_eq!(links[0], "/some_random_crate/latest/some_random_crate/");
        Ok(())
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn search() -> Result<()> {
        let mut crates_io = mockito::Server::new_async().await;

        let env = TestEnvironment::builder()
            .registry_api_config(
                docs_rs_registry_api::Config::builder()
                    .registry_api_host(crates_io.url().parse().unwrap())
                    .build(),
            )
            .build()
            .await?;

        let web = env.web_app().await;
        env.fake_release()
            .await
            .name("some_random_crate")
            .version("2.0.0")
            .create()
            .await?;
        env.fake_release()
            .await
            .name("some_random_crate")
            .version("1.0.0")
            .create()
            .await?;

        env.fake_release()
            .await
            .name("and_another_one")
            .version("0.0.1")
            .create()
            .await?;

        env.fake_release()
            .await
            .name("yet_another_crate")
            .version("0.1.0")
            .yanked(true)
            .create()
            .await?;

        // release with only in-progress build (= in progress release) will not be shown
        env.fake_release()
            .await
            .name("in_progress")
            .version("0.1.0")
            .builds(vec![
                FakeBuild::default()
                    .build_status(BuildStatus::InProgress)
                    .rustc_version("rustc (blabla 2022-01-01)")
                    .docsrs_version("docs.rs 4.0.0"),
            ])
            .create()
            .await?;

        // release that failed in the fetch-step, will miss some details
        let mut conn = env.async_conn().await?;
        fake_release_that_failed_before_build(
            &mut conn,
            "failed_hard",
            "0.1.0",
            "some random error",
        )
        .await?;

        let _m = crates_io
            .mock("GET", "/api/v1/crates")
            .match_query(Matcher::AllOf(vec![
                Matcher::UrlEncoded("q".into(), "some_random_crate".into()),
                Matcher::UrlEncoded("per_page".into(), "30".into()),
            ]))
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(
                json!({
                    "crates": [
                        { "name": "some_random_crate" },
                        { "name": "some_other_crate" },
                        { "name": "and_another_one" },
                        { "name": "yet_another_crate" },
                        { "name": "in_progress" },
                        { "name": "failed_hard" }
                    ],
                    "meta": {
                        "next_page": null,
                        "prev_page": null,
                    }
                })
                .to_string(),
            )
            .create_async()
            .await;

        let links = get_release_links("/releases/search?query=some_random_crate", &web).await?;

        // `some_other_crate` won't be shown since we don't have it yet
        assert_eq!(links.len(), 4);
        // * `max_version` from the crates.io search result will be ignored since we
        //   might not have it yet, or the doc-build might be in progress.
        // * ranking/order from crates.io result is preserved
        // * version used is the highest semver following our own "latest version" logic
        assert_eq!(links[0], "/some_random_crate/latest/some_random_crate/");
        assert_eq!(links[1], "/and_another_one/latest/and_another_one/");
        assert_eq!(links[2], "/yet_another_crate/0.1.0/yet_another_crate/");
        assert_eq!(links[3], "/crate/failed_hard/0.1.0");
        Ok(())
    }

    async fn get_release_links(path: &str, web: &axum::Router) -> Result<Vec<String>, Error> {
        let response = web.get(path).await?;
        assert!(response.status().is_success());

        let page = kuchikiki::parse_html().one(response.text().await?);

        Ok(page
            .select("a.release")
            .expect("missing heading")
            .map(|el| {
                let attributes = el.attributes.borrow();
                attributes.get("href").unwrap().to_string()
            })
            .collect())
    }

    #[test]
    fn releases_by_stars() {
        async_wrapper(|env| async move {
            env.fake_release()
                .await
                .name("crate_that_succeeded_with_github")
                .version("0.1.0")
                .github_stats("some/repo", 66, 22, 11)
                .release_time(Utc.with_ymd_and_hms(2020, 4, 16, 4, 33, 50).unwrap())
                .create()
                .await?;

            env.fake_release()
                .await
                .name("crate_that_succeeded_with_github")
                .version("0.2.0")
                .github_stats("some/repo", 66, 22, 11)
                .release_time(Utc.with_ymd_and_hms(2020, 4, 20, 4, 33, 50).unwrap())
                .create()
                .await?;

            env.fake_release()
                .await
                .name("crate_that_succeeded_without_github")
                .release_time(Utc.with_ymd_and_hms(2020, 5, 16, 4, 33, 50).unwrap())
                .version("0.2.0")
                .create()
                .await?;

            env.fake_release()
                .await
                .name("crate_that_failed_with_github")
                .version("0.1.0")
                .github_stats("some/repo", 33, 22, 11)
                .release_time(Utc.with_ymd_and_hms(2020, 6, 16, 4, 33, 50).unwrap())
                .build_result_failed()
                .create()
                .await?;

            let links = get_release_links("/releases/stars", &env.web_app().await).await?;

            // output is sorted by stars, not release-time
            assert_eq!(links.len(), 2);
            assert_eq!(
                links[0],
                "/crate_that_succeeded_with_github/0.2.0/crate_that_succeeded_with_github/"
            );
            assert_eq!(links[1], "/crate/crate_that_failed_with_github/0.1.0");

            Ok(())
        })
    }

    #[test]
    fn failures_by_stars() {
        async_wrapper(|env| async move {
            env.fake_release()
                .await
                .name("crate_that_succeeded_with_github")
                .version("0.1.0")
                .github_stats("some/repo", 66, 22, 11)
                .release_time(Utc.with_ymd_and_hms(2020, 4, 16, 4, 33, 50).unwrap())
                .create()
                .await?;

            env.fake_release()
                .await
                .name("crate_that_succeeded_with_github")
                .version("0.2.0")
                .github_stats("some/repo", 66, 22, 11)
                .release_time(Utc.with_ymd_and_hms(2020, 4, 20, 4, 33, 50).unwrap())
                .create()
                .await?;

            env.fake_release()
                .await
                .name("crate_that_succeeded_without_github")
                .release_time(Utc.with_ymd_and_hms(2020, 5, 16, 4, 33, 50).unwrap())
                .version("0.2.0")
                .create()
                .await?;

            env.fake_release()
                .await
                .name("crate_that_failed_with_github")
                .version("0.1.0")
                .github_stats("some/repo", 33, 22, 11)
                .release_time(Utc.with_ymd_and_hms(2020, 6, 16, 4, 33, 50).unwrap())
                .build_result_failed()
                .create()
                .await?;

            let links = get_release_links("/releases/failures", &env.web_app().await).await?;

            // output is sorted by stars, not release-time
            assert_eq!(links.len(), 1);
            assert_eq!(links[0], "/crate/crate_that_failed_with_github/0.1.0");

            Ok(())
        })
    }

    #[test]
    fn releases_failed_by_time() {
        async_wrapper(|env| async move {
            env.fake_release()
                .await
                .name("crate_that_succeeded_with_github")
                .version("0.1.0")
                .github_stats("some/repo", 33, 22, 11)
                .release_time(Utc.with_ymd_and_hms(2020, 4, 16, 4, 33, 50).unwrap())
                .create()
                .await?;
            // make sure that crates get at most one release shown, so they don't crowd the page
            env.fake_release()
                .await
                .name("crate_that_succeeded_with_github")
                .github_stats("some/repo", 33, 22, 11)
                .release_time(Utc.with_ymd_and_hms(2020, 5, 16, 4, 33, 50).unwrap())
                .version("0.2.0")
                .create()
                .await?;
            env.fake_release()
                .await
                .name("crate_that_failed")
                .version("0.1.0")
                .release_time(Utc.with_ymd_and_hms(2020, 6, 16, 4, 33, 50).unwrap())
                .build_result_failed()
                .create()
                .await?;

            let links =
                get_release_links("/releases/recent-failures", &env.web_app().await).await?;

            assert_eq!(links.len(), 1);
            assert_eq!(links[0], "/crate/crate_that_failed/0.1.0");

            Ok(())
        })
    }

    #[test]
    fn releases_homepage_and_recent() {
        async_wrapper(|env| async move {
            env.fake_release()
                .await
                .name("crate_that_succeeded_with_github")
                .version("0.1.0")
                .github_stats("some/repo", 33, 22, 11)
                .release_time(Utc.with_ymd_and_hms(2020, 4, 16, 4, 33, 50).unwrap())
                .create()
                .await?;
            env.fake_release()
                .await
                .name("crate_that_succeeded_with_github")
                .version("0.2.0-rc")
                .github_stats("some/repo", 33, 22, 11)
                .release_time(Utc.with_ymd_and_hms(2020, 4, 16, 8, 33, 50).unwrap())
                .build_result_failed()
                .create()
                .await?;
            env.fake_release()
                .await
                .name("crate_that_succeeded_with_github")
                .github_stats("some/repo", 33, 22, 11)
                .release_time(Utc.with_ymd_and_hms(2020, 5, 16, 4, 33, 50).unwrap())
                .version("0.2.0")
                .create()
                .await?;
            env.fake_release()
                .await
                .name("crate_that_failed")
                .version("0.1.0")
                .release_time(Utc.with_ymd_and_hms(2020, 6, 16, 4, 33, 50).unwrap())
                .build_result_failed()
                .create()
                .await?;

            // make sure that crates get at most one release shown, so they don't crowd the homepage
            assert_eq!(
                get_release_links("/", &env.web_app().await).await?,
                [
                    "/crate/crate_that_failed/0.1.0",
                    "/crate_that_succeeded_with_github/0.2.0/crate_that_succeeded_with_github/",
                ]
            );

            // but on the main release list they all show, including prerelease
            assert_eq!(
                get_release_links("/releases", &env.web_app().await).await?,
                [
                    "/crate/crate_that_failed/0.1.0",
                    "/crate_that_succeeded_with_github/0.2.0/crate_that_succeeded_with_github/",
                    "/crate/crate_that_succeeded_with_github/0.2.0-rc",
                    "/crate_that_succeeded_with_github/0.1.0/crate_that_succeeded_with_github/",
                ]
            );

            Ok(())
        })
    }

    #[test]
    fn release_activity() {
        async_wrapper(|env| async move {
            let web = env.web_app().await;

            let empty_data = format!("data: [{}]", vec!["0"; 30].join(", "));

            // no data / only zeros without releases
            let response = web.get("/releases/activity").await?;
            assert!(response.status().is_success());
            let text = response.text().await?;
            assert_eq!(text.matches(&empty_data).count(), 2);

            env.fake_release()
                .await
                .name("some_random_crate")
                .create()
                .await?;
            env.fake_release()
                .await
                .name("some_random_crate_that_failed")
                .build_result_failed()
                .create()
                .await?;

            // same when the release is on the current day, since we ignore today.
            let response = web.get("/releases/activity").await?;
            assert!(response.status().is_success());
            assert_eq!(response.text().await?.matches(&empty_data).count(), 2);

            env.fake_release()
                .await
                .name("some_random_crate_yesterday")
                .release_time(Utc::now() - Duration::try_days(1).unwrap())
                .create()
                .await?;
            env.fake_release()
                .await
                .name("some_random_crate_that_failed_yesterday")
                .build_result_failed()
                .release_time(Utc::now() - Duration::try_days(1).unwrap())
                .create()
                .await?;

            // with releases yesterday we get the data we want
            let response = web.get("/releases/activity").await?;
            assert!(response.status().is_success());
            let text = response.text().await?;
            // counts contain both releases
            assert!(text.contains(&format!("data: [{}, 2]", vec!["0"; 29].join(", "))));
            // failures only one
            assert!(text.contains(&format!("data: [{}, 1]", vec!["0"; 29].join(", "))));

            Ok(())
        })
    }

    #[test]
    fn release_feed() {
        async_wrapper(|env| async move {
            let web = env.web_app().await;
            web.assert_success("/releases/feed").await?;

            env.fake_release()
                .await
                .name("some_random_crate")
                .create()
                .await?;
            env.fake_release()
                .await
                .name("some_random_crate_that_failed")
                .build_result_failed()
                .create()
                .await?;
            web.assert_success("/releases/feed").await?;
            Ok(())
        })
    }

    #[test]
    fn test_releases_queue() {
        async_wrapper(|env| async move {
            let web = env.web_app().await;

            let empty =
                kuchikiki::parse_html().one(web.get("/releases/queue").await?.text().await?);
            assert!(
                empty
                    .select(".queue-list > strong")
                    .expect("missing heading")
                    .any(|el| el.text_contents().contains("nothing"))
            );

            assert!(
                !empty
                    .select(".release > strong")
                    .expect("missing heading")
                    .any(|el| el.text_contents().contains("active CDN deployments"))
            );

            let queue = env.build_queue()?;
            queue.add_crate(&FOO, &V1, 0, None).await?;
            queue.add_crate(&BAR, &V2, -10, None).await?;
            queue.add_crate(&BAZ, &V3, 10, None).await?;

            let full = kuchikiki::parse_html().one(web.get("/releases/queue").await?.text().await?);
            let items = full
                .select(".queue-list > li")
                .expect("missing list items")
                .collect::<Vec<_>>();

            assert_eq!(items.len(), 3);
            let expected = [
                ("bar", V2, Some(10)),
                ("foo", V1, None),
                ("baz", V3, Some(-10)),
            ];
            for (li, expected) in items.iter().zip(&expected) {
                let a = li.as_node().select_first("a").expect("missing link");
                assert!(a.text_contents().contains(expected.0));
                assert!(a.text_contents().contains(&expected.1.to_string()));

                if let Some(priority) = expected.2 {
                    assert!(
                        li.text_contents()
                            .contains(&format!("priority: {priority}"))
                    );
                }
            }

            Ok(())
        });
    }

    #[test]
    fn test_releases_queue_in_progress() {
        async_wrapper(|env| async move {
            let web = env.web_app().await;

            // we have two queued releases, where the build for one is already in progress
            let queue = env.build_queue()?;
            queue.add_crate(&FOO, &V1, 0, None).await?;
            queue.add_crate(&BAR, &V2, 0, None).await?;

            env.fake_release()
                .await
                .name("foo")
                .version(V1)
                .builds(vec![
                    FakeBuild::default()
                        .build_status(BuildStatus::InProgress)
                        .rustc_version("rustc (blabla 2022-01-01)")
                        .docsrs_version("docs.rs 4.0.0"),
                ])
                .create()
                .await?;

            let full = kuchikiki::parse_html().one(web.get("/releases/queue").await?.text().await?);

            let lists = full
                .select(".queue-list")
                .expect("missing queues")
                .collect::<Vec<_>>();
            assert_eq!(lists.len(), 2);

            let in_progress_items: Vec<_> = lists[0]
                .as_node()
                .select("li > a")
                .expect("missing in progress list items")
                .map(|node| node.text_contents().trim().to_string())
                .collect();
            assert_eq!(in_progress_items, vec![format!("foo {V1}")]);

            let queued_items: Vec<_> = lists[1]
                .as_node()
                .select("li > a")
                .expect("missing queued list items")
                .map(|node| node.text_contents().trim().to_string())
                .collect();
            assert_eq!(queued_items, vec![format!("bar {V2}")]);

            Ok(())
        });
    }

    #[test]
    fn test_releases_rebuild_queue_empty() {
        async_wrapper(|env| async move {
            let web = env.web_app().await;

            let empty =
                kuchikiki::parse_html().one(web.get("/releases/queue").await?.text().await?);

            assert!(
                empty
                    .select(".about > p")
                    .expect("missing heading")
                    .any(|el| el.text_contents().contains("We continuously rebuild"))
            );

            assert!(
                empty
                    .select(".about > p")
                    .expect("missing heading")
                    .any(|el| el.text_contents().contains("crates in the rebuild queue"))
            );

            Ok(())
        });
    }

    #[test]
    fn test_releases_rebuild_queue_with_crates() {
        async_wrapper(|env| async move {
            let web = env.web_app().await;
            let queue = env.build_queue()?;
            queue
                .add_crate(&FOO, &V1, PRIORITY_CONTINUOUS, None)
                .await?;
            queue
                .add_crate(&BAR, &V2, PRIORITY_CONTINUOUS + 1, None)
                .await?;
            queue
                .add_crate(&BAZ, &V3, PRIORITY_CONTINUOUS - 1, None)
                .await?;

            let full = kuchikiki::parse_html().one(web.get("/releases/queue").await?.text().await?);
            let items = full
                .select(".rebuild-queue-list > li")
                .expect("missing list items")
                .collect::<Vec<_>>();

            // empty because expand_rebuild_queue is not set
            assert_eq!(items.len(), 0);
            assert!(
                full.select(".about > p")
                    .expect("missing heading")
                    .any(|el| el
                        .text_contents()
                        .contains("There are currently 2 crates in the rebuild queue"))
            );

            let full = kuchikiki::parse_html()
                .one(web.get("/releases/queue?expand=1").await?.text().await?);
            let build_queue_list = full
                .select(".queue-list > li")
                .expect("missing list items")
                .collect::<Vec<_>>();
            let rebuild_queue_list = full
                .select(".rebuild-queue-list > li")
                .expect("missing list items")
                .collect::<Vec<_>>();

            assert_eq!(build_queue_list.len(), 1);
            assert_eq!(rebuild_queue_list.len(), 2);
            assert!(
                rebuild_queue_list
                    .iter()
                    .any(|li| li.text_contents().contains("foo"))
            );
            assert!(
                rebuild_queue_list
                    .iter()
                    .any(|li| li.text_contents().contains("bar"))
            );
            assert!(
                build_queue_list
                    .iter()
                    .any(|li| li.text_contents().contains("baz"))
            );
            assert!(
                !rebuild_queue_list
                    .iter()
                    .any(|li| li.text_contents().contains("baz"))
            );

            Ok(())
        });
    }

    #[test]
    fn home_page_links() {
        async_wrapper(|env| async move {
            let web = env.web_app().await;
            env.fake_release()
                .await
                .name("some_random_crate")
                .add_owner(CrateOwner {
                    login: "foobar".into(),
                    avatar: "https://example.org/foobar".into(),
                    kind: OwnerKind::User,
                })
                .create()
                .await?;

            let mut urls = vec![];
            let mut seen = HashSet::new();
            seen.insert("".to_owned());

            let resp = web.get("/").await?;
            resp.assert_cache_control(CachePolicy::ShortInCdnAndBrowser, env.config());

            assert!(resp.status().is_success());

            let html = kuchikiki::parse_html().one(resp.text().await?);
            for link in html.select("a").unwrap() {
                let link = link.as_node().as_element().unwrap();

                urls.push(link.attributes.borrow().get("href").unwrap().to_owned());
            }

            while let Some(url) = urls.pop() {
                // Skip urls we've already checked
                if !seen.insert(url.clone()) {
                    continue;
                }

                let resp =
                    if url.starts_with("http://") || url.starts_with("https://") || url == "#" {
                        // Skip external links
                        continue;
                    } else {
                        web.get(&url).await?
                    };
                let status = resp.status();
                assert!(
                    status.is_success(),
                    "failed to GET {url}: {status}, {:?}",
                    resp.headers().get("Location"),
                );
            }

            Ok(())
        });
    }

    #[test]
    fn check_releases_page_content() {
        // NOTE: this is a little fragile and may have to be updated if the HTML layout changes
        let sel = ".pure-menu-horizontal>.pure-menu-list>.pure-menu-item>.pure-menu-link>.title";
        async_wrapper(|env| async move {
            for url in &[
                "/releases",
                "/releases/stars",
                "/releases/recent-failures",
                "/releases/failures",
                "/releases/activity",
                "/releases/queue",
            ] {
                let page = kuchikiki::parse_html()
                    .one(env.web_app().await.get(url).await.unwrap().text().await?);
                assert_eq!(page.select("#crate-title").unwrap().count(), 1);
                let not_matching = page
                    .select(sel)
                    .unwrap()
                    .map(|node| node.text_contents())
                    .zip(
                        [
                            "Recent",
                            "Stars",
                            "Recent Failures",
                            "Failures By Stars",
                            "Activity",
                            "Queue",
                        ]
                        .iter(),
                    )
                    .filter(|(a, b)| a.as_str() != **b)
                    .collect::<Vec<_>>();
                if !not_matching.is_empty() {
                    let not_found = not_matching.iter().map(|(_, b)| b).collect::<Vec<_>>();
                    let found = not_matching.iter().map(|(a, _)| a).collect::<Vec<_>>();
                    assert!(
                        not_matching.is_empty(),
                        "Titles did not match for URL `{url}`: not found: {not_found:?}, found: {found:?}",
                    );
                }
            }

            Ok(())
        });
    }

    #[test]
    fn check_owner_releases_redirect() {
        async_wrapper(|env| async move {
            let web = env.web_app().await;

            web.assert_redirect_unchecked("/releases/someone", "https://crates.io/users/someone")
                .await?;
            Ok(())
        });
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn crates_not_on_docsrs() -> Result<()> {
        let mut crates_io = mockito::Server::new_async().await;

        let env = TestEnvironment::builder()
            .registry_api_config(
                docs_rs_registry_api::Config::builder()
                    .registry_api_host(crates_io.url().parse().unwrap())
                    .build(),
            )
            .build()
            .await?;

        let web = env.web_app().await;
        env.fake_release()
            .await
            .name("some_random_crate")
            .create()
            .await?;

        let _m = crates_io
            .mock("GET", "/api/v1/crates")
            .match_query(Matcher::AllOf(vec![
                Matcher::UrlEncoded("q".into(), "some_random_crate".into()),
                Matcher::UrlEncoded("per_page".into(), "30".into()),
            ]))
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(
                json!({
                    "crates": [
                        { "name": "some_random_crate" },
                        { "name": "some_random_crate2" },
                        { "name": "some_random_crate3" },
                    ],
                    "meta": {
                        "next_page": "null",
                        "prev_page": "null",
                    }
                })
                .to_string(),
            )
            .create_async()
            .await;

        let response = web.get("/releases/search?query=some_random_crate").await?;
        assert!(response.status().is_success());

        let page = kuchikiki::parse_html().one(response.text().await?);

        assert_eq!(page.select("div.name.not-available").unwrap().count(), 2);
        assert_eq!(
            page.select("div.name:not(.not-available)").unwrap().count(),
            1
        );

        Ok(())
    }

    #[test]
    fn recent_failures_correct_pagination_links() {
        async_wrapper(|env| async move {
            for i in 0..RELEASES_IN_RELEASES + 1 {
                env.fake_release()
                    .await
                    .name("failed")
                    .version(format!("0.0.{i}"))
                    .build_result_failed()
                    .create()
                    .await?;
            }

            let web = env.web_app().await;

            let response = web.get("/releases/recent-failures").await?;
            assert!(response.status().is_success());

            let page = kuchikiki::parse_html().one(response.text().await?);
            assert_eq!(
                page.select("div.description")
                    .unwrap()
                    .next()
                    .unwrap()
                    .text_contents(),
                "Recent crates failed to build"
            );

            let next_page_link = page.select("div.pagination > a").unwrap().next().unwrap();
            assert_eq!(next_page_link.text_contents().trim(), "Next Page");

            let next_page_url = next_page_link
                .attributes
                .borrow()
                .get("href")
                .unwrap()
                .to_owned();
            assert_eq!(next_page_url, "/releases/recent-failures/2");

            let response = web.get(&next_page_url).await?;
            assert!(response.status().is_success());

            let page = kuchikiki::parse_html().one(response.text().await?);
            assert_eq!(
                page.select("div.description")
                    .unwrap()
                    .next()
                    .unwrap()
                    .text_contents(),
                "Recent crates failed to build"
            );
            assert_eq!(
                page.select(".recent-releases-container > ul > li .name")
                    .unwrap()
                    .next()
                    .unwrap()
                    .text_contents()
                    .trim(),
                "failed-0.0.0"
            );

            Ok(())
        });
    }

    #[test]
    fn test_search_std() {
        async_wrapper(|env| async move {
            let web = env.web_app().await;

            async fn inner(web: &axum::Router, krate: &str) -> Result<(), anyhow::Error> {
                let full = kuchikiki::parse_html().one(
                    web.get(&format!("/releases/search?query={krate}"))
                        .await?
                        .text()
                        .await?,
                );
                let items = full
                    .select("ul a.release")
                    .expect("missing list items")
                    .collect::<Vec<_>>();

                // empty because expand_rebuild_queue is not set
                let item_element = items.first().unwrap();
                let item = item_element.as_node();
                assert_eq!(
                    item.select(".name")
                        .unwrap()
                        .next()
                        .unwrap()
                        .text_contents(),
                    "std"
                );
                assert_eq!(
                    item.select(".description")
                        .unwrap()
                        .next()
                        .unwrap()
                        .text_contents(),
                    "Rust standard library",
                );
                assert_eq!(
                    item_element.attributes.borrow().get("href").unwrap(),
                    "https://doc.rust-lang.org/stable/std/"
                );

                Ok(())
            }

            inner(&web, "std").await?;
            inner(&web, "libstd").await?;

            Ok(())
        });
    }
}
