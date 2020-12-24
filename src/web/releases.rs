//! Releases web handlers

use crate::{
    build_queue::QueuedCrate,
    db::Pool,
    impl_webpage,
    web::{error::Nope, match_version, page::WebPage, redirect_base},
    BuildQueue,
};
use chrono::{DateTime, NaiveDateTime, Utc};
use iron::{
    headers::{ContentType, Expires, HttpDate},
    mime::{Mime, SubLevel, TopLevel},
    modifiers::Redirect,
    status, IronResult, Request, Response, Url,
};
use postgres::Client;
use router::Router;
use serde::Serialize;
use serde_json::Value;

/// Number of release in home page
const RELEASES_IN_HOME: i64 = 15;
/// Releases in /releases page
const RELEASES_IN_RELEASES: i64 = 30;
/// Releases in recent releases feed
const RELEASES_IN_FEED: i64 = 150;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Release {
    pub(crate) name: String,
    pub(crate) version: String,
    description: Option<String>,
    target_name: Option<String>,
    rustdoc_status: bool,
    pub(crate) release_time: DateTime<Utc>,
    stars: i32,
}

impl Default for Release {
    fn default() -> Release {
        Release {
            name: String::new(),
            version: String::new(),
            description: None,
            target_name: None,
            rustdoc_status: false,
            release_time: Utc::now(),
            stars: 0,
        }
    }
}

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub(crate) enum Order {
    ReleaseTime, // this is default order
    GithubStars,
    RecentFailures,
    FailuresByGithubStars,
}

impl Default for Order {
    fn default() -> Self {
        Self::ReleaseTime
    }
}

pub(crate) fn get_releases(conn: &mut Client, page: i64, limit: i64, order: Order) -> Vec<Release> {
    let offset = (page - 1) * limit;

    // WARNING: it is _crucial_ that this always be hard-coded and NEVER be user input
    let (ordering, filter_failed): (&'static str, _) = match order {
        Order::ReleaseTime => ("releases.release_time", false),
        Order::GithubStars => ("github_repos.stars", false),
        Order::RecentFailures => ("releases.release_time", true),
        Order::FailuresByGithubStars => ("github_repos.stars", true),
    };
    let query = format!(
        "SELECT crates.name,
            releases.version,
            releases.description,
            releases.target_name,
            releases.release_time,
            releases.rustdoc_status,
            github_repos.stars
        FROM crates
        INNER JOIN releases ON crates.id = releases.crate_id
        LEFT JOIN github_repos ON releases.github_repo = github_repos.id
        WHERE
            ((NOT $3) OR (releases.build_status = FALSE AND releases.is_library = TRUE))
            AND crates.latest_version_id = releases.id
        ORDER BY {} DESC NULLS LAST
        LIMIT $1 OFFSET $2",
        ordering,
    );

    conn.query(query.as_str(), &[&limit, &offset, &filter_failed])
        .unwrap()
        .into_iter()
        .map(|row| Release {
            name: row.get(0),
            version: row.get(1),
            description: row.get(2),
            target_name: row.get(3),
            release_time: DateTime::from_utc(row.get::<_, NaiveDateTime>(4), Utc),
            rustdoc_status: row.get(5),
            stars: row.get::<_, Option<i32>>(6).unwrap_or(0),
        })
        .collect()
}

fn get_releases_by_author(
    conn: &mut Client,
    page: i64,
    limit: i64,
    author: &str,
) -> (String, Vec<Release>) {
    let offset = (page - 1) * limit;

    let query = "
        SELECT crates.name,
               releases.version,
               releases.description,
               releases.target_name,
               releases.release_time,
               releases.rustdoc_status,
               github_repos.stars,
               authors.name
        FROM crates
        INNER JOIN releases ON releases.id = crates.latest_version_id
        INNER JOIN author_rels ON releases.id = author_rels.rid
        INNER JOIN authors ON authors.id = author_rels.aid
        LEFT JOIN github_repos ON releases.github_repo = github_repos.id
        WHERE authors.slug = $1
        ORDER BY github_repos.stars DESC NULLS LAST
        LIMIT $2 OFFSET $3";
    let query = conn.query(query, &[&author, &limit, &offset]).unwrap();

    let mut author_name = None;
    let packages = query
        .into_iter()
        .map(|row| {
            if author_name.is_none() {
                author_name = Some(row.get(7));
            }

            Release {
                name: row.get(0),
                version: row.get(1),
                description: row.get(2),
                target_name: row.get(3),
                release_time: DateTime::from_utc(row.get::<_, NaiveDateTime>(4), Utc),
                rustdoc_status: row.get(5),
                stars: row.get::<_, Option<i32>>(6).unwrap_or(0),
            }
        })
        .collect();

    (author_name.unwrap_or_default(), packages)
}

fn get_releases_by_owner(
    conn: &mut Client,
    page: i64,
    limit: i64,
    author: &str,
) -> (String, Vec<Release>) {
    let offset = (page - 1) * limit;

    let query = "SELECT crates.name,
                        releases.version,
                        releases.description,
                        releases.target_name,
                        releases.release_time,
                        releases.rustdoc_status,
                        github_repos.stars,
                        owners.name,
                        owners.login
                 FROM crates
                 INNER JOIN releases ON releases.id = crates.latest_version_id
                 INNER JOIN owner_rels ON owner_rels.cid = crates.id
                 INNER JOIN owners ON owners.id = owner_rels.oid
                 LEFT JOIN github_repos ON releases.github_repo = github_repos.id
                 WHERE owners.login = $1
                 ORDER BY github_repos.stars DESC NULLS LAST
                 LIMIT $2 OFFSET $3";
    let query = conn.query(query, &[&author, &limit, &offset]).unwrap();

    let mut author_name = None;
    let packages = query
        .into_iter()
        .map(|row| {
            if author_name.is_none() {
                author_name = Some(if !row.get::<usize, String>(7).is_empty() {
                    row.get(7)
                } else {
                    row.get(8)
                });
            }

            Release {
                name: row.get(0),
                version: row.get(1),
                description: row.get(2),
                target_name: row.get(3),
                release_time: DateTime::from_utc(row.get::<_, NaiveDateTime>(4), Utc),
                rustdoc_status: row.get(5),
                stars: row.get::<_, Option<i32>>(6).unwrap_or(0),
            }
        })
        .collect();

    (author_name.unwrap_or_default(), packages)
}

/// Get the search results for a crate search query
///
/// Retrieves crates which names have a levenshtein distance of less than or equal to 3,
/// crates who fit into or otherwise are made up of the query or crates whose descriptions
/// match the search query.
///
/// * `query`: The query string, unfiltered
/// * `page`: The page of results to show (1-indexed)
/// * `limit`: The number of results to return
///
/// Returns 0 and an empty Vec when no results are found or if a database error occurs
///
fn get_search_results(
    conn: &mut Client,
    mut query: &str,
    page: i64,
    limit: i64,
) -> Result<(i64, Vec<Release>), failure::Error> {
    query = query.trim();
    if query.is_empty() {
        return Ok((0, Vec::new()));
    }
    let offset = (page - 1) * limit;

    let statement = "
        SELECT
            crates.name AS name,
            releases.version AS version,
            releases.description AS description,
            releases.target_name AS target_name,
            releases.release_time AS release_time,
            releases.rustdoc_status AS rustdoc_status,
            github_repos.stars AS github_stars,
            COUNT(*) OVER() as total
        FROM crates
        INNER JOIN (
            SELECT releases.id, releases.crate_id
            FROM (
                SELECT
                    releases.id,
                    releases.crate_id,
                    RANK() OVER (PARTITION BY crate_id ORDER BY release_time DESC) as rank
                FROM releases
                WHERE releases.rustdoc_status AND NOT releases.yanked
            ) AS releases
            WHERE releases.rank = 1
        ) AS latest_release ON latest_release.crate_id = crates.id
        INNER JOIN releases ON latest_release.id = releases.id
        LEFT JOIN github_repos ON releases.github_repo = github_repos.id
        WHERE
            ((char_length($1)::float - levenshtein(crates.name, $1)::float) / char_length($1)::float) >= 0.65
            OR crates.name ILIKE CONCAT('%', $1, '%')
        GROUP BY crates.id, releases.id, github_repos.stars
        ORDER BY
            levenshtein(crates.name, $1) ASC,
            crates.name ILIKE CONCAT('%', $1, '%'),
            releases.downloads DESC
        LIMIT $2 OFFSET $3";

    let rows = conn.query(statement, &[&query, &limit, &offset])?;

    // Each row contains the total number of possible/valid results, just get it once
    let total_results = rows
        .get(0)
        .map(|row| row.get::<_, i64>("total"))
        .unwrap_or_default();
    let packages: Vec<Release> = rows
        .into_iter()
        .map(|row| Release {
            name: row.get("name"),
            version: row.get("version"),
            description: row.get("description"),
            target_name: row.get("target_name"),
            release_time: DateTime::from_utc(row.get("release_time"), Utc),
            rustdoc_status: row.get("rustdoc_status"),
            stars: row.get::<_, Option<i32>>("github_stars").unwrap_or(0),
        })
        .collect();

    Ok((total_results, packages))
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct HomePage {
    recent_releases: Vec<Release>,
}

impl_webpage! {
    HomePage = "core/home.html",
}

pub fn home_page(req: &mut Request) -> IronResult<Response> {
    let mut conn = extension!(req, Pool).get()?;
    let recent_releases = get_releases(&mut conn, 1, RELEASES_IN_HOME, Order::ReleaseTime);

    HomePage { recent_releases }.into_response(req)
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct ReleaseFeed {
    recent_releases: Vec<Release>,
}

impl_webpage! {
    ReleaseFeed  = "releases/feed.xml",
    content_type = ContentType(Mime(TopLevel::Application, SubLevel::Xml, vec![])),
}

pub fn releases_feed_handler(req: &mut Request) -> IronResult<Response> {
    let mut conn = extension!(req, Pool).get()?;
    let recent_releases = get_releases(&mut conn, 1, RELEASES_IN_FEED, Order::ReleaseTime);

    ReleaseFeed { recent_releases }.into_response(req)
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct ViewReleases {
    releases: Vec<Release>,
    description: String,
    release_type: ReleaseType,
    show_next_page: bool,
    show_previous_page: bool,
    page_number: i64,
    author: Option<String>,
}

impl_webpage! {
    ViewReleases = "releases/releases.html",
}

#[derive(Debug, Copy, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub(super) enum ReleaseType {
    Recent,
    Stars,
    RecentFailures,
    Failures,
    Author,
    Search,
}

fn releases_handler(req: &mut Request, release_type: ReleaseType) -> IronResult<Response> {
    let page_number: i64 = extension!(req, Router)
        .find("page")
        .and_then(|page_num| page_num.parse().ok())
        .unwrap_or(1);

    let (description, release_order) = match release_type {
        ReleaseType::Recent => ("Recently uploaded crates", Order::ReleaseTime),
        ReleaseType::Stars => ("Crates with most stars", Order::GithubStars),
        ReleaseType::RecentFailures => ("Recent crates failed to build", Order::RecentFailures),
        ReleaseType::Failures => (
            "Crates with most stars failed to build",
            Order::FailuresByGithubStars,
        ),

        ReleaseType::Author | ReleaseType::Search => panic!(
            "The authors and search page have special requirements and cannot use this handler",
        ),
    };

    let releases = {
        let mut conn = extension!(req, Pool).get()?;
        get_releases(&mut conn, page_number, RELEASES_IN_RELEASES, release_order)
    };

    // Show next and previous page buttons
    let (show_next_page, show_previous_page) = (
        releases.len() == RELEASES_IN_RELEASES as usize,
        page_number != 1,
    );

    ViewReleases {
        releases,
        description: description.into(),
        release_type,
        show_next_page,
        show_previous_page,
        page_number,
        author: None,
    }
    .into_response(req)
}

pub fn recent_releases_handler(req: &mut Request) -> IronResult<Response> {
    releases_handler(req, ReleaseType::Recent)
}

pub fn releases_by_stars_handler(req: &mut Request) -> IronResult<Response> {
    releases_handler(req, ReleaseType::Stars)
}

pub fn releases_recent_failures_handler(req: &mut Request) -> IronResult<Response> {
    releases_handler(req, ReleaseType::RecentFailures)
}

pub fn releases_failures_by_stars_handler(req: &mut Request) -> IronResult<Response> {
    releases_handler(req, ReleaseType::Failures)
}

pub fn author_handler(req: &mut Request) -> IronResult<Response> {
    let router = extension!(req, Router);
    // page number of releases
    let page_number: i64 = router
        .find("page")
        .and_then(|page_num| page_num.parse().ok())
        .unwrap_or(1);
    let author = router
        .find("author")
        // TODO: Accurate error here, the author wasn't provided
        .ok_or(Nope::CrateNotFound)?;

    let (author_name, releases) = {
        let mut conn = extension!(req, Pool).get()?;

        if author.starts_with('@') {
            let mut author = author.split('@');

            get_releases_by_owner(
                &mut conn,
                page_number,
                RELEASES_IN_RELEASES,
                // TODO: Is this fallible?
                cexpect!(req, author.nth(1)),
            )
        } else {
            get_releases_by_author(&mut conn, page_number, RELEASES_IN_RELEASES, author)
        }
    };

    if releases.is_empty() {
        // TODO: Accurate error here, the author wasn't found
        return Err(Nope::CrateNotFound.into());
    }

    // Show next and previous page buttons
    let (show_next_page, show_previous_page) = (
        releases.len() == RELEASES_IN_RELEASES as usize,
        page_number != 1,
    );

    ViewReleases {
        releases,
        description: format!("Crates from {}", author_name),
        release_type: ReleaseType::Author,
        show_next_page,
        show_previous_page,
        page_number,
        author: Some(author.into()),
    }
    .into_response(req)
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub(super) struct Search {
    pub(super) title: String,
    #[serde(rename = "releases")]
    pub(super) results: Vec<Release>,
    pub(super) search_query: Option<String>,
    pub(super) previous_page_button: bool,
    pub(super) next_page_button: bool,
    pub(super) current_page: i64,
    /// This should always be `ReleaseType::Search`
    pub(super) release_type: ReleaseType,
    #[serde(skip)]
    pub(super) status: iron::status::Status,
}

impl Default for Search {
    fn default() -> Self {
        Self {
            title: String::default(),
            results: Vec::default(),
            search_query: None,
            previous_page_button: false,
            next_page_button: false,
            current_page: 0,
            release_type: ReleaseType::Search,
            status: iron::status::Ok,
        }
    }
}

impl_webpage! {
    Search = "releases/releases.html",
    status = |search| search.status,
}

pub fn search_handler(req: &mut Request) -> IronResult<Response> {
    let url = req.url.as_ref();
    let mut params = url.query_pairs();
    let query = params.find(|(key, _)| key == "query");
    let mut conn = extension!(req, Pool).get()?;

    if let Some((_, query)) = query {
        // check if I am feeling lucky button pressed and redirect user to crate page
        // if there is a match
        // TODO: Redirecting to latest doc might be more useful
        // NOTE: calls `query_pairs()` again because iterators are lazy and only yield items once
        if url
            .query_pairs()
            .any(|(key, _)| key == "i-am-feeling-lucky")
        {
            // redirect to a random crate if query is empty
            if query.is_empty() {
                // FIXME: This is a fast query but using a constant
                //        There are currently 280 crates with docs and 100+
                //        starts. This should be fine for a while.
                let rows = ctry!(
                    req,
                    conn.query(
                        "SELECT crates.name,
                            releases.version,
                            releases.target_name
                     FROM crates
                     INNER JOIN releases
                         ON crates.latest_version_id = releases.id
                     WHERE github_stars >= 100 AND rustdoc_status = true
                     OFFSET FLOOR(RANDOM() * 280) LIMIT 1",
                        &[]
                    ),
                );
                let row = rows.into_iter().next().unwrap();

                let name: String = row.get("name");
                let version: String = row.get("version");
                let target_name: String = row.get("target_name");
                let url = ctry!(
                    req,
                    Url::parse(&format!(
                        "{}/{}/{}/{}",
                        redirect_base(req),
                        name,
                        version,
                        target_name
                    )),
                );

                let mut resp = Response::with((status::Found, Redirect(url)));
                resp.headers.set(Expires(HttpDate(time::now())));

                return Ok(resp);
            }

            // since we never pass a version into `match_version` here, we'll never get
            // `MatchVersion::Exact`, so the distinction between `Exact` and `Semver` doesn't
            // matter
            if let Ok(matchver) = match_version(&mut conn, &query, None) {
                let (version, id) = matchver.version.into_parts();
                let query = matchver.corrected_name.unwrap_or_else(|| query.to_string());

                // FIXME: This is a super dirty way to check if crate have rustdocs generated.
                //        match_version should handle this instead of this code block.
                //        This block is introduced to fix #163
                let rustdoc_status = {
                    let rows = ctry!(
                        req,
                        conn.query(
                            "SELECT rustdoc_status
                         FROM releases
                         WHERE releases.id = $1",
                            &[&id]
                        ),
                    );

                    rows.into_iter()
                        .next()
                        .map(|r| r.get("rustdoc_status"))
                        .unwrap_or_default()
                };

                let url = if rustdoc_status {
                    ctry!(
                        req,
                        Url::parse(&format!("{}/{}/{}", redirect_base(req), query, version)),
                    )
                } else {
                    ctry!(
                        req,
                        Url::parse(&format!(
                            "{}/crate/{}/{}",
                            redirect_base(req),
                            query,
                            version,
                        )),
                    )
                };

                let mut resp = Response::with((status::Found, Redirect(url)));
                resp.headers.set(Expires(HttpDate(time::now())));

                return Ok(resp);
            }
        }

        let (_, results) = ctry!(
            req,
            get_search_results(&mut conn, &query, 1, RELEASES_IN_RELEASES)
        );
        let title = if results.is_empty() {
            format!("No results found for '{}'", query)
        } else {
            format!("Search results for '{}'", query)
        };

        // FIXME: There is no pagination
        Search {
            title,
            results,
            search_query: Some(query.into_owned()),
            ..Default::default()
        }
        .into_response(req)
    } else {
        Err(Nope::NoResults.into())
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
struct ReleaseActivity {
    description: &'static str,
    activity_data: Value,
}

impl_webpage! {
    ReleaseActivity = "releases/activity.html",
}

pub fn activity_handler(req: &mut Request) -> IronResult<Response> {
    let mut conn = extension!(req, Pool).get()?;
    let activity_data: Value = ctry!(
        req,
        conn.query(
            "SELECT value FROM config WHERE name = 'release_activity'",
            &[]
        ),
    )
    .iter()
    .next()
    .map_or(Value::Null, |row| row.get("value"));

    ReleaseActivity {
        description: "Monthly release activity",
        activity_data,
    }
    .into_response(req)
}

#[derive(Debug, Clone, PartialEq, Serialize)]
struct BuildQueuePage {
    description: &'static str,
    queue: Vec<QueuedCrate>,
}

impl_webpage! {
    BuildQueuePage = "releases/build_queue.html",
}

pub fn build_queue_handler(req: &mut Request) -> IronResult<Response> {
    let mut queue = ctry!(req, extension!(req, BuildQueue).queued_crates());
    for krate in queue.iter_mut() {
        // The priority here is inverted: in the database if a crate has a higher priority it
        // will be built after everything else, which is counter-intuitive for people not
        // familiar with docs.rs's inner workings.
        krate.priority = -krate.priority;
    }

    BuildQueuePage {
        description: "List of crates scheduled to build",
        queue,
    }
    .into_response(req)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test::{assert_success, wrapper, TestEnvironment};
    use chrono::TimeZone;
    use failure::Error;
    use kuchiki::traits::TendrilSink;
    use std::collections::HashSet;

    #[test]
    fn releases_by_stars() {
        wrapper(|env| {
            let db = env.db();

            env.fake_release()
                .name("foo")
                .version("1.0.0")
                .github_stats("ghost/foo", 10, 10, 10)
                .create()?;
            env.fake_release()
                .name("bar")
                .version("1.0.0")
                .github_stats("ghost/bar", 20, 20, 20)
                .create()?;
            env.fake_release().name("baz").version("1.0.0").create()?;

            let releases = get_releases(&mut db.conn(), 1, 10, Order::GithubStars);
            assert_eq!(
                vec![
                    "bar", // 20 stars
                    "foo", // 10 stars
                    "baz", // no stars (still included at the bottom)
                ],
                releases
                    .iter()
                    .map(|release| release.name.as_str())
                    .collect::<Vec<_>>(),
            );

            Ok(())
        })
    }

    #[test]
    fn database_search() {
        wrapper(|env| {
            let db = env.db();

            env.fake_release().name("foo").version("0.0.0").create()?;
            env.fake_release()
                .name("bar-foo")
                .version("0.0.0")
                .create()?;
            env.fake_release()
                .name("foo-bar")
                .version("0.0.1")
                .create()?;
            env.fake_release().name("fo0").version("0.0.0").create()?;
            env.fake_release()
                .name("fool")
                .version("0.0.0")
                .build_result_successful(false)
                .create()?;
            env.fake_release()
                .name("freakin")
                .version("0.0.0")
                .create()?;
            env.fake_release()
                .name("something unreleated")
                .version("0.0.0")
                .create()?;

            let (num_results, results) = get_search_results(&mut db.conn(), "foo", 1, 100)?;
            assert_eq!(num_results, 4);

            let mut results = results.into_iter();

            let expected = ["foo", "fo0", "bar-foo", "foo-bar"];
            for expected in expected.iter() {
                assert_eq!(expected, &results.next().unwrap().name);
            }
            assert_eq!(results.count(), 0);

            Ok(())
        })
    }

    #[test]
    fn exacts_dont_care() {
        wrapper(|env| {
            let db = env.db();

            let releases = ["regex", "regex-", "regex-syntax"];
            for release in releases.iter() {
                env.fake_release().name(release).version("0.0.0").create()?;
            }

            let near_matches = ["Regex", "rEgex", "reGex", "regEx", "regeX"];

            for name in near_matches.iter() {
                let (num_results, mut results) =
                    dbg!(get_search_results(&mut db.conn(), *name, 1, 100))?;
                assert_eq!(num_results, 3);

                for name in releases.iter() {
                    assert_eq!(results.remove(0).name, *name);
                }
                assert!(results.is_empty());
            }

            Ok(())
        })
    }

    #[test]
    fn unsuccessful_not_shown() {
        wrapper(|env| {
            let db = env.db();
            env.fake_release()
                .name("regex")
                .version("0.0.0")
                .build_result_successful(false)
                .create()?;

            let (num_results, results) = get_search_results(&mut db.conn(), "regex", 1, 100)?;
            assert_eq!(num_results, 0);

            let results = results.into_iter();
            assert_eq!(results.count(), 0);

            Ok(())
        })
    }

    #[test]
    fn yanked_not_shown() {
        wrapper(|env| {
            let db = env.db();
            env.fake_release()
                .name("regex")
                .version("0.0.0")
                .yanked(true)
                .create()?;

            let (num_results, results) = get_search_results(&mut db.conn(), "regex", 1, 100)?;
            assert_eq!(num_results, 0);

            let results = results.into_iter();
            assert_eq!(results.count(), 0);

            Ok(())
        })
    }

    #[test]
    fn fuzzily_match() {
        wrapper(|env| {
            let db = env.db();
            env.fake_release().name("regex").version("0.0.0").create()?;

            let (num_results, results) = get_search_results(&mut db.conn(), "redex", 1, 100)?;
            assert_eq!(num_results, 1);

            let mut results = results.into_iter();
            assert_eq!(results.next().unwrap().name, "regex");
            assert_eq!(results.count(), 0);

            Ok(())
        })
    }

    // Description searching more than doubles search time
    // #[test]
    // fn search_descriptions() {
    //     wrapper(|env| {
    //         let db = env.db();
    //         env.fake_release()
    //             .name("something_completely_unrelated")
    //             .description("Supercalifragilisticexpialidocious")
    //             .create()?;
    //
    //         let (num_results, results) =
    //             get_search_results(&mut db.conn(), "supercalifragilisticexpialidocious", 1, 100)?;
    //         assert_eq!(num_results, 1);
    //
    //         let mut results = results.into_iter();
    //         assert_eq!(
    //             results.next().unwrap().name,
    //             "something_completely_unrelated"
    //         );
    //         assert_eq!(results.count(), 0);
    //
    //         Ok(())
    //     })
    // }

    #[test]
    fn search_limits() {
        wrapper(|env| {
            let db = env.db();

            env.fake_release().name("something_magical").create()?;
            env.fake_release().name("something_sinister").create()?;
            env.fake_release().name("something_fantastical").create()?;
            env.fake_release()
                .name("something_completely_unrelated")
                .create()?;

            let (num_results, results) = get_search_results(&mut db.conn(), "something", 1, 2)?;
            assert_eq!(num_results, 4);

            let mut results = results.into_iter();
            assert_eq!(results.next().unwrap().name, "something_magical");
            assert_eq!(results.next().unwrap().name, "something_sinister");
            assert_eq!(results.count(), 0);

            Ok(())
        })
    }

    #[test]
    fn search_offsets() {
        wrapper(|env| {
            let db = env.db();
            env.fake_release().name("something_magical").create()?;
            env.fake_release().name("something_sinister").create()?;
            env.fake_release().name("something_fantastical").create()?;
            env.fake_release()
                .name("something_completely_unrelated")
                .create()?;

            let (num_results, results) = get_search_results(&mut db.conn(), "something", 2, 2)?;
            assert_eq!(num_results, 4);

            let mut results = results.into_iter();
            assert_eq!(results.next().unwrap().name, "something_fantastical");
            assert_eq!(
                results.next().unwrap().name,
                "something_completely_unrelated",
            );
            assert_eq!(results.count(), 0);

            Ok(())
        })
    }

    #[test]
    fn release_dates() {
        wrapper(|env| {
            let db = env.db();
            env.fake_release()
                .name("somethang")
                .release_time(Utc.ymd(2021, 4, 16).and_hms(4, 33, 50))
                .version("0.3.0")
                .description("this is the correct choice")
                .create()?;
            env.fake_release()
                .name("somethang")
                .release_time(Utc.ymd(2020, 4, 16).and_hms(4, 33, 50))
                .description("second")
                .version("0.2.0")
                .create()?;
            env.fake_release()
                .name("somethang")
                .release_time(Utc.ymd(2019, 4, 16).and_hms(4, 33, 50))
                .description("third")
                .version("0.1.0")
                .create()?;
            env.fake_release()
                .name("somethang")
                .release_time(Utc.ymd(2018, 4, 16).and_hms(4, 33, 50))
                .description("fourth")
                .version("0.0.0")
                .create()?;

            let (num_results, results) = get_search_results(&mut db.conn(), "somethang", 1, 100)?;
            assert_eq!(num_results, 1);

            let mut results = results.into_iter();
            assert_eq!(
                results.next().unwrap().description,
                Some("this is the correct choice".into()),
            );
            assert_eq!(results.count(), 0);

            Ok(())
        })
    }

    // Description searching more than doubles search time
    // #[test]
    // fn fuzzy_over_description() {
    //     wrapper(|env| {
    //         let db = env.db();
    //         env.fake_release()
    //             .name("name_better_than_description")
    //             .description("this is the correct choice")
    //             .create()?;
    //         env.fake_release()
    //             .name("im_completely_unrelated")
    //             .description("name_better_than_description")
    //             .create()?;
    //         env.fake_release()
    //             .name("i_have_zero_relation_whatsoever")
    //             .create()?;
    //
    //         let (num_results, results) =
    //             get_search_results(&mut db.conn(), "name_better_than_description", 1, 100)?;
    //         assert_eq!(num_results, 2);
    //
    //         let mut results = results.into_iter();
    //
    //         let next = results.next().unwrap();
    //         assert_eq!(next.name, "name_better_than_description");
    //         assert_eq!(next.description, Some("this is the correct choice".into()));
    //
    //         let next = results.next().unwrap();
    //         assert_eq!(next.name, "im_completely_unrelated");
    //         assert_eq!(
    //             next.description,
    //             Some("name_better_than_description".into())
    //         );
    //
    //         assert_eq!(results.count(), 0);
    //
    //         Ok(())
    //     })
    // }

    #[test]
    fn dont_return_unrelated() {
        wrapper(|env| {
            let db = env.db();
            env.fake_release().name("match").create()?;
            env.fake_release().name("matcher").create()?;
            env.fake_release().name("matchest").create()?;
            env.fake_release()
                .name("i_am_useless_and_mean_nothing")
                .create()?;

            let (num_results, results) = get_search_results(&mut db.conn(), "match", 1, 100)?;
            assert_eq!(num_results, 3);

            let mut results = results.into_iter();
            assert_eq!(results.next().unwrap().name, "match");
            assert_eq!(results.next().unwrap().name, "matcher");
            assert_eq!(results.next().unwrap().name, "matchest");
            assert_eq!(results.count(), 0);

            Ok(())
        })
    }

    #[test]
    fn order_by_downloads() {
        wrapper(|env| {
            let db = env.db();
            env.fake_release().name("matca").downloads(100).create()?;
            env.fake_release().name("matcb").downloads(10).create()?;
            env.fake_release().name("matcc").downloads(1).create()?;

            let (num_results, results) = get_search_results(&mut db.conn(), "match", 1, 100)?;
            assert_eq!(num_results, 3);

            let mut results = results.into_iter();
            assert_eq!(results.next().unwrap().name, "matca");
            assert_eq!(results.next().unwrap().name, "matcb");
            assert_eq!(results.next().unwrap().name, "matcc");
            assert_eq!(results.count(), 0);

            Ok(())
        })
    }

    fn releases_link_test(path: &str, env: &TestEnvironment) -> Result<(), Error> {
        env.fake_release()
            .name("crate_that_succeeded")
            .version("0.1.0")
            .create()?;
        // make sure that crates get at most one release shown, so they don't crowd the page
        env.fake_release()
            .name("crate_that_succeeded")
            .version("0.2.0")
            .create()?;
        env.fake_release()
            .name("crate_that_failed")
            .version("0.1.0")
            .build_result_successful(false)
            .create()?;
        let page = kuchiki::parse_html().one(env.frontend().get(path).send()?.text()?);
        let releases: Vec<_> = page.select("a.release").expect("missing heading").collect();
        if path.contains("failures") {
            assert_eq!(
                1,
                releases.len(),
                "expected one failed release for path {}",
                path
            );
        } else {
            assert_eq!(2, releases.len(), "expected 2 releases for path {}", path);
        }
        for release in releases {
            let attributes = release.attributes.borrow();
            let url = attributes.get("href").unwrap();
            if url.contains("crate_that_succeeded") {
                assert_eq!(
                    url, "/crate_that_succeeded/0.2.0/crate_that_succeeded",
                    "for path {}",
                    path
                );
            } else {
                assert_eq!(url, "/crate/crate_that_failed/0.1.0", "for path {}", path);
            }
        }
        Ok(())
    }

    #[test]
    fn search() {
        wrapper(|env| {
            let web = env.frontend();
            env.fake_release().name("some_random_crate").create()?;
            assert_success("/releases/search?query=some_random_crate", web)
        })
    }

    #[test]
    fn releases() {
        wrapper(|env| {
            let web = env.frontend();
            for page in &[
                "/",
                "/releases",
                "/releases/stars",
                "/releases/recent-failures",
                "/releases/failures",
            ] {
                assert_success(page, web)?;
                releases_link_test(page, env)?;
            }
            Ok(())
        })
    }

    #[test]
    fn release_activity() {
        wrapper(|env| {
            let web = env.frontend();
            assert_success("/releases/activity", web)?;
            Ok(())
        })
    }

    #[test]
    fn release_feed() {
        wrapper(|env| {
            let web = env.frontend();
            assert_success("/releases/feed", web)?;

            env.fake_release().name("some_random_crate").create()?;
            env.fake_release()
                .name("some_random_crate_that_failed")
                .build_result_successful(false)
                .create()?;
            assert_success("/releases/feed", web)
        })
    }

    #[test]
    fn test_releases_queue() {
        wrapper(|env| {
            let queue = env.build_queue();
            let web = env.frontend();

            let empty = kuchiki::parse_html().one(web.get("/releases/queue").send()?.text()?);
            assert!(empty
                .select(".release > strong")
                .expect("missing heading")
                .any(|el| el.text_contents().contains("nothing")));

            queue.add_crate("foo", "1.0.0", 0, None)?;
            queue.add_crate("bar", "0.1.0", -10, None)?;
            queue.add_crate("baz", "0.0.1", 10, None)?;

            let full = kuchiki::parse_html().one(web.get("/releases/queue").send()?.text()?);
            let items = full
                .select(".queue-list > li")
                .expect("missing list items")
                .collect::<Vec<_>>();

            assert_eq!(items.len(), 3);
            let expected = [
                ("bar", "0.1.0", Some(10)),
                ("foo", "1.0.0", None),
                ("baz", "0.0.1", Some(-10)),
            ];
            for (li, expected) in items.iter().zip(&expected) {
                let a = li.as_node().select_first("a").expect("missing link");
                assert!(a.text_contents().contains(expected.0));
                assert!(a.text_contents().contains(expected.1));

                if let Some(priority) = expected.2 {
                    assert!(li
                        .text_contents()
                        .contains(&format!("priority: {}", priority)));
                }
            }

            Ok(())
        });
    }

    #[test]
    fn authors_page() {
        wrapper(|env| {
            let web = env.frontend();
            env.fake_release()
                .name("some_random_crate")
                .author("frankenstein <frankie@stein.com>")
                .create()?;
            assert_success("/releases/frankenstein", web)
        })
    }

    #[test]
    fn home_page_links() {
        wrapper(|env| {
            let web = env.frontend();
            env.fake_release()
                .name("some_random_crate")
                .author("frankenstein <frankie@stein.com>")
                .create()?;

            let mut urls = vec![];
            let mut seen = HashSet::new();
            seen.insert("".to_owned());

            let resp = web.get("").send()?;
            assert!(resp.status().is_success());

            let html = kuchiki::parse_html().one(resp.text()?);
            for link in html.select("a").unwrap() {
                let link = link.as_node().as_element().unwrap();

                urls.push(link.attributes.borrow().get("href").unwrap().to_owned());
            }

            while let Some(url) = urls.pop() {
                // Skip urls we've already checked
                if !seen.insert(url.clone()) {
                    continue;
                }

                let resp = if url.starts_with("http://") || url.starts_with("https://") {
                    // Skip external links
                    continue;
                } else {
                    web.get(&url).send()?
                };
                let status = resp.status();
                assert!(status.is_success(), "failed to GET {}: {}", url, status);
            }

            Ok(())
        });
    }

    #[test]
    fn check_releases_page_content() {
        // NOTE: this is a little fragile and may have to be updated if the HTML layout changes
        let sel = ".pure-menu-horizontal>.pure-menu-list>.pure-menu-item>.pure-menu-link>.title";
        wrapper(|env| {
            let tester = |url| {
                let page = kuchiki::parse_html()
                    .one(env.frontend().get(url).send().unwrap().text().unwrap());
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
                        "Titles did not match for URL `{}`: not found: {:?}, found: {:?}",
                        url,
                        not_found,
                        found,
                    );
                }
            };

            for url in &[
                "/releases",
                "/releases/stars",
                "/releases/recent-failures",
                "/releases/failures",
                "/releases/activity",
                "/releases/queue",
            ] {
                tester(url);
            }

            Ok(())
        });
    }

    #[test]
    fn test_empty_query() {
        wrapper(|env| {
            let mut conn = env.db().conn();
            let (num_results, results) = get_search_results(&mut conn, "", 0, 0).unwrap();
            assert_eq!(num_results, 0);
            assert!(results.is_empty());
            Ok(())
        })
    }
}
