//! Releases web handlers

use super::{
    error::Nope,
    match_version,
    page::{
        HomePage, ReleaseActivity, ReleaseFeed, ReleaseQueue, ReleaseType, Search, ViewReleases,
        WebPage,
    },
    pool::Pool,
    redirect_base,
};
use iron::prelude::*;
use iron::status;
use postgres::Connection;
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
    #[serde(serialize_with = "super::rfc3339")]
    pub(crate) release_time: time::Timespec,
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
            release_time: time::get_time(),
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

pub(crate) fn get_releases(conn: &Connection, page: i64, limit: i64, order: Order) -> Vec<Release> {
    let offset = (page - 1) * limit;

    // TODO: This function changed so much during development and current version have code
    //       repeats for queries. There is definitely room for improvements.
    let query = match order {
        Order::ReleaseTime => {
            "SELECT crates.name,
                    releases.version,
                    releases.description,
                    releases.target_name,
                    releases.release_time,
                    releases.rustdoc_status,
                    crates.github_stars
             FROM crates
             INNER JOIN releases ON crates.id = releases.crate_id
             ORDER BY releases.release_time DESC
             LIMIT $1 OFFSET $2"
        }
        Order::GithubStars => {
            "SELECT crates.name,
                    releases.version,
                    releases.description,
                    releases.target_name,
                    releases.release_time,
                    releases.rustdoc_status,
                    crates.github_stars
             FROM crates
             INNER JOIN releases ON releases.id = crates.latest_version_id
             ORDER BY crates.github_stars DESC
             LIMIT $1 OFFSET $2"
        }
        Order::RecentFailures => {
            "SELECT crates.name,
                    releases.version,
                    releases.description,
                    releases.target_name,
                    releases.release_time,
                    releases.rustdoc_status,
                    crates.github_stars
             FROM crates
             INNER JOIN releases ON crates.id = releases.crate_id
             WHERE releases.build_status = FALSE AND releases.is_library = TRUE
             ORDER BY releases.release_time DESC
             LIMIT $1 OFFSET $2"
        }
        Order::FailuresByGithubStars => {
            "SELECT crates.name,
                    releases.version,
                    releases.description,
                    releases.target_name,
                    releases.release_time,
                    releases.rustdoc_status,
                    crates.github_stars
             FROM crates
             INNER JOIN releases ON releases.id = crates.latest_version_id
             WHERE releases.build_status = FALSE AND releases.is_library = TRUE
             ORDER BY crates.github_stars DESC
             LIMIT $1 OFFSET $2"
        }
    };
    let query = conn.query(&query, &[&limit, &offset]).unwrap();

    query
        .into_iter()
        .map(|row| Release {
            name: row.get(0),
            version: row.get(1),
            description: row.get(2),
            target_name: row.get(3),
            release_time: row.get(4),
            rustdoc_status: row.get(5),
            stars: row.get(6),
        })
        .collect()
}

fn get_releases_by_author(
    conn: &Connection,
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
               crates.github_stars,
               authors.name
        FROM crates
        INNER JOIN releases ON releases.id = crates.latest_version_id
        INNER JOIN author_rels ON releases.id = author_rels.rid
        INNER JOIN authors ON authors.id = author_rels.aid
        WHERE authors.slug = $1
        ORDER BY crates.github_stars DESC
        LIMIT $2 OFFSET $3";
    let query = conn.query(&query, &[&author, &limit, &offset]).unwrap();

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
                release_time: row.get(4),
                rustdoc_status: row.get(5),
                stars: row.get(6),
            }
        })
        .collect();

    (author_name.unwrap_or_default(), packages)
}

fn get_releases_by_owner(
    conn: &Connection,
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
                        crates.github_stars,
                        owners.name,
                        owners.login
                 FROM crates
                 INNER JOIN releases ON releases.id = crates.latest_version_id
                 INNER JOIN owner_rels ON owner_rels.cid = crates.id
                 INNER JOIN owners ON owners.id = owner_rels.oid
                 WHERE owners.login = $1
                 ORDER BY crates.github_stars DESC
                 LIMIT $2 OFFSET $3";
    let query = conn.query(&query, &[&author, &limit, &offset]).unwrap();

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
                release_time: row.get(4),
                rustdoc_status: row.get(5),
                stars: row.get(6),
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
    conn: &Connection,
    mut query: &str,
    page: i64,
    limit: i64,
) -> (i64, Vec<Release>) {
    query = query.trim();
    let offset = (page - 1) * limit;

    let statement = "
        SELECT
            crates.name AS name,
            releases.version AS version,
            releases.description AS description,
            releases.target_name AS target_name,
            releases.release_time AS release_time,
            releases.rustdoc_status AS rustdoc_status,
            crates.github_stars AS github_stars,
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
        WHERE
            ((char_length($1)::float - levenshtein(crates.name, $1)::float) / char_length($1)::float) >= 0.65
            OR crates.name ILIKE CONCAT('%', $1, '%')
        GROUP BY crates.id, releases.id
        ORDER BY
            levenshtein(crates.name, $1) ASC,
            crates.name ILIKE CONCAT('%', $1, '%'),
            releases.downloads DESC
        LIMIT $2 OFFSET $3";

    let rows = if let Ok(rows) = conn.query(statement, &[&query, &limit, &offset]) {
        rows
    } else {
        return (0, Vec::new());
    };

    // Each row contains the total number of possible/valid results, just get it once
    let total_results = rows
        .iter()
        .next()
        .map(|row| row.get::<_, i64>("total"))
        .unwrap_or_default();
    let packages: Vec<Release> = rows
        .into_iter()
        .map(|row| Release {
            name: row.get("name"),
            version: row.get("version"),
            description: row.get("description"),
            target_name: row.get("target_name"),
            release_time: row.get("release_time"),
            rustdoc_status: row.get("rustdoc_status"),
            stars: row.get::<_, i32>("github_stars"),
        })
        .collect();

    (total_results, packages)
}

pub fn home_page(req: &mut Request) -> IronResult<Response> {
    let conn = extension!(req, Pool).get()?;
    let recent_releases = get_releases(&conn, 1, RELEASES_IN_HOME, Order::ReleaseTime);

    HomePage { recent_releases }.into_response()
}

pub fn releases_feed_handler(req: &mut Request) -> IronResult<Response> {
    let conn = extension!(req, Pool).get()?;
    let recent_releases = get_releases(&conn, 1, RELEASES_IN_FEED, Order::ReleaseTime);

    ReleaseFeed { recent_releases }.into_response()
}

fn releases_handler(
    releases: Vec<Release>,
    page_number: i64,
    release_type: ReleaseType,
    description: impl Into<String>,
) -> IronResult<Response> {
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
    .into_response()
}

// Following functions caused a code repeat due to design of our /releases/ URL routes
pub fn recent_releases_handler(req: &mut Request) -> IronResult<Response> {
    let page_number: i64 = extension!(req, Router)
        .find("page")
        .unwrap_or("1")
        .parse()
        .unwrap_or(1);
    let conn = extension!(req, Pool).get()?;
    let packages = get_releases(&conn, page_number, RELEASES_IN_RELEASES, Order::ReleaseTime);

    releases_handler(
        packages,
        page_number,
        ReleaseType::Recent,
        "Recently uploaded crates",
    )
}

pub fn releases_by_stars_handler(req: &mut Request) -> IronResult<Response> {
    let page_number: i64 = extension!(req, Router)
        .find("page")
        .unwrap_or("1")
        .parse()
        .unwrap_or(1);
    let conn = extension!(req, Pool).get()?;
    let packages = get_releases(&conn, page_number, RELEASES_IN_RELEASES, Order::GithubStars);

    releases_handler(
        packages,
        page_number,
        ReleaseType::Stars,
        "Crates with most stars",
    )
}

pub fn releases_recent_failures_handler(req: &mut Request) -> IronResult<Response> {
    let page_number: i64 = extension!(req, Router)
        .find("page")
        .unwrap_or("1")
        .parse()
        .unwrap_or(1);
    let conn = extension!(req, Pool).get()?;
    let packages = get_releases(
        &conn,
        page_number,
        RELEASES_IN_RELEASES,
        Order::RecentFailures,
    );

    releases_handler(
        packages,
        page_number,
        ReleaseType::RecentFailures,
        "Recent crates failed to build",
    )
}

pub fn releases_failures_by_stars_handler(req: &mut Request) -> IronResult<Response> {
    let page_number: i64 = extension!(req, Router)
        .find("page")
        .unwrap_or("1")
        .parse()
        .unwrap_or(1);
    let conn = extension!(req, Pool).get()?;
    let packages = get_releases(
        &conn,
        page_number,
        RELEASES_IN_RELEASES,
        Order::FailuresByGithubStars,
    );

    releases_handler(
        packages,
        page_number,
        ReleaseType::Failures,
        "Crates with most stars failed to build",
    )
}

pub fn author_handler(req: &mut Request) -> IronResult<Response> {
    let router = extension!(req, Router);
    // page number of releases
    let page_number: i64 = router.find("page").unwrap_or("1").parse().unwrap_or(1);

    let conn = extension!(req, Pool).get()?;

    let author = ctry!(router
        .find("author")
        .ok_or_else(|| IronError::new(Nope::CrateNotFound, status::NotFound)));

    let (author_name, releases) = if author.starts_with('@') {
        let mut author = author.split('@');

        get_releases_by_owner(
            &conn,
            page_number,
            RELEASES_IN_RELEASES,
            cexpect!(author.nth(1)),
        )
    } else {
        get_releases_by_author(&conn, page_number, RELEASES_IN_RELEASES, author)
    };

    if releases.is_empty() {
        return Err(IronError::new(Nope::CrateNotFound, status::NotFound));
    }

    // Show next and previous page buttons
    // This is a temporary solution to avoid expensive COUNT(*)
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
        author: Some(author_name.to_owned()),
    }
    .into_response()
}

pub fn search_handler(req: &mut Request) -> IronResult<Response> {
    use params::{Params, Value};

    let params = ctry!(req.get::<Params>());
    let query = params.find(&["query"]);

    let conn = extension!(req, Pool).get()?;
    if let Some(Value::String(query)) = query {
        // check if I am feeling lucky button pressed and redirect user to crate page
        // if there is a match
        // TODO: Redirecting to latest doc might be more useful
        if params.find(&["i-am-feeling-lucky"]).is_some() {
            use iron::modifiers::Redirect;
            use iron::Url;

            // redirect to a random crate if query is empty
            if query.is_empty() {
                let rows = ctry!(conn.query(
                    "SELECT crates.name,
                            releases.version,
                            releases.target_name
                     FROM crates
                     INNER JOIN releases
                         ON crates.latest_version_id = releases.id
                     WHERE github_stars >= 100 AND rustdoc_status = true
                     OFFSET FLOOR(RANDOM() * 280) LIMIT 1",
                    &[]
                ));
                //                                        ~~~~~~^
                // FIXME: This is a fast query but using a constant
                //        There are currently 280 crates with docs and 100+
                //        starts. This should be fine for a while.
                let name: String = rows.get(0).get(0);
                let version: String = rows.get(0).get(1);
                let target_name: String = rows.get(0).get(2);
                let url = ctry!(Url::parse(&format!(
                    "{}/{}/{}/{}",
                    redirect_base(req),
                    name,
                    version,
                    target_name
                )));

                let mut resp = Response::with((status::Found, Redirect(url)));
                use iron::headers::{Expires, HttpDate};
                resp.headers.set(Expires(HttpDate(time::now())));
                return Ok(resp);
            }

            // since we never pass a version into `match_version` here, we'll never get
            // `MatchVersion::Exact`, so the distinction between `Exact` and `Semver` doesn't
            // matter
            if let Some(matchver) = match_version(&conn, &query, None) {
                let (version, id) = matchver.version.into_parts();
                let query = matchver.corrected_name.unwrap_or_else(|| query.to_string());
                // FIXME: This is a super dirty way to check if crate have rustdocs generated.
                //        match_version should handle this instead of this code block.
                //        This block is introduced to fix #163
                let rustdoc_status = {
                    let rows = ctry!(conn.query(
                        "SELECT rustdoc_status
                                                 FROM releases
                                                 WHERE releases.id = $1",
                        &[&id]
                    ));
                    if rows.is_empty() {
                        false
                    } else {
                        rows.get(0).get(0)
                    }
                };
                let url = if rustdoc_status {
                    ctry!(Url::parse(
                        &format!("{}/{}/{}", redirect_base(req), query, version)[..]
                    ))
                } else {
                    ctry!(Url::parse(
                        &format!("{}/crate/{}/{}", redirect_base(req), query, version)[..]
                    ))
                };
                let mut resp = Response::with((status::Found, Redirect(url)));

                use iron::headers::{Expires, HttpDate};
                resp.headers.set(Expires(HttpDate(time::now())));
                return Ok(resp);
            }
        }

        let (_, results) = get_search_results(&conn, &query, 1, RELEASES_IN_RELEASES);
        let title = if results.is_empty() {
            format!("No results found for '{}'", query)
        } else {
            format!("Search results for '{}'", query)
        };

        // FIXME: There is no pagination
        Search {
            title: title,
            results,
            search_query: Some(query.to_owned()),
            ..Default::default()
        }
        .into_response()
    } else {
        Err(IronError::new(Nope::NoResults, status::NotFound))
    }
}

pub fn activity_handler(req: &mut Request) -> IronResult<Response> {
    let conn = extension!(req, Pool).get()?;
    let activity_data: Value = ctry!(conn.query(
        "SELECT value FROM config WHERE name = 'release_activity'",
        &[]
    ))
    .get(0)
    .get(0);

    ReleaseActivity {
        description: "Monthly release activity".to_owned(),
        activity_data,
    }
    .into_response()
}

pub fn build_queue_handler(req: &mut Request) -> IronResult<Response> {
    let conn = extension!(req, Pool).get()?;
    let query = conn
        .query(
            "SELECT name, version, priority
             FROM queue
             WHERE attempt < 5
             ORDER BY priority ASC, attempt ASC, id ASC",
            &[],
        )
        .unwrap();

    let queue: Vec<(String, String, i32)> = query
        .iter()
        .map(|krate| {
            (
                krate.get("name"),
                krate.get("version"),
                // The priority here is inverted: in the database if a crate has a higher priority it
                // will be built after everything else, which is counter-intuitive for people not
                // familiar with docs.rs's inner workings.
                -krate.get::<_, i32>("priority"),
            )
        })
        .collect();

    ReleaseQueue {
        description: "List of crates scheduled to build".to_owned(),
        queue_is_empty: queue.is_empty(),
        queue,
    }
    .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test::{assert_success, wrapper};
    use serde_json::json;

    #[test]
    fn database_search() {
        wrapper(|env| {
            let db = env.db();

            db.fake_release().name("foo").version("0.0.0").create()?;
            db.fake_release()
                .name("bar-foo")
                .version("0.0.0")
                .create()?;
            db.fake_release()
                .name("foo-bar")
                .version("0.0.1")
                .create()?;
            db.fake_release().name("fo0").version("0.0.0").create()?;
            db.fake_release()
                .name("fool")
                .version("0.0.0")
                .build_result_successful(false)
                .create()?;
            db.fake_release()
                .name("freakin")
                .version("0.0.0")
                .create()?;
            db.fake_release()
                .name("something unreleated")
                .version("0.0.0")
                .create()?;

            let (num_results, results) = get_search_results(&db.conn(), "foo", 1, 100);
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
                db.fake_release().name(release).version("0.0.0").create()?;
            }

            let near_matches = ["Regex", "rEgex", "reGex", "regEx", "regeX"];

            for name in near_matches.iter() {
                let (num_results, mut results) = get_search_results(&db.conn(), *name, 1, 100);
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
            db.fake_release()
                .name("regex")
                .version("0.0.0")
                .build_result_successful(false)
                .create()?;

            let (num_results, results) = get_search_results(&db.conn(), "regex", 1, 100);
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
            db.fake_release()
                .name("regex")
                .version("0.0.0")
                .yanked(true)
                .create()?;

            let (num_results, results) = get_search_results(&db.conn(), "regex", 1, 100);
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
            db.fake_release().name("regex").version("0.0.0").create()?;

            let (num_results, results) = get_search_results(&db.conn(), "redex", 1, 100);
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
    //         db.fake_release()
    //             .name("something_completely_unrelated")
    //             .description("Supercalifragilisticexpialidocious")
    //             .create()?;
    //
    //         let (num_results, results) =
    //             get_search_results(&db.conn(), "supercalifragilisticexpialidocious", 1, 100);
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

            db.fake_release().name("something_magical").create()?;
            db.fake_release().name("something_sinister").create()?;
            db.fake_release().name("something_fantastical").create()?;
            db.fake_release()
                .name("something_completely_unrelated")
                .create()?;

            let (num_results, results) = get_search_results(&db.conn(), "something", 1, 2);
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
            db.fake_release().name("something_magical").create()?;
            db.fake_release().name("something_sinister").create()?;
            db.fake_release().name("something_fantastical").create()?;
            db.fake_release()
                .name("something_completely_unrelated")
                .create()?;

            let (num_results, results) = get_search_results(&db.conn(), "something", 2, 2);
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
            db.fake_release()
                .name("somethang")
                .release_time(time::Timespec::new(1000, 0))
                .version("0.3.0")
                .description("this is the correct choice")
                .create()?;
            db.fake_release()
                .name("somethang")
                .release_time(time::Timespec::new(100, 0))
                .description("second")
                .version("0.2.0")
                .create()?;
            db.fake_release()
                .name("somethang")
                .release_time(time::Timespec::new(10, 0))
                .description("third")
                .version("0.1.0")
                .create()?;
            db.fake_release()
                .name("somethang")
                .release_time(time::Timespec::new(1, 0))
                .description("fourth")
                .version("0.0.0")
                .create()?;

            let (num_results, results) = get_search_results(&db.conn(), "somethang", 1, 100);
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
    //         db.fake_release()
    //             .name("name_better_than_description")
    //             .description("this is the correct choice")
    //             .create()?;
    //         db.fake_release()
    //             .name("im_completely_unrelated")
    //             .description("name_better_than_description")
    //             .create()?;
    //         db.fake_release()
    //             .name("i_have_zero_relation_whatsoever")
    //             .create()?;
    //
    //         let (num_results, results) =
    //             get_search_results(&db.conn(), "name_better_than_description", 1, 100);
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
            db.fake_release().name("match").create()?;
            db.fake_release().name("matcher").create()?;
            db.fake_release().name("matchest").create()?;
            db.fake_release()
                .name("i_am_useless_and_mean_nothing")
                .create()?;

            let (num_results, results) = get_search_results(&db.conn(), "match", 1, 100);
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
            db.fake_release().name("matca").downloads(100).create()?;
            db.fake_release().name("matcb").downloads(10).create()?;
            db.fake_release().name("matcc").downloads(1).create()?;

            let (num_results, results) = get_search_results(&db.conn(), "match", 1, 100);
            assert_eq!(num_results, 3);

            let mut results = results.into_iter();
            assert_eq!(results.next().unwrap().name, "matca");
            assert_eq!(results.next().unwrap().name, "matcb");
            assert_eq!(results.next().unwrap().name, "matcc");
            assert_eq!(results.count(), 0);

            Ok(())
        })
    }

    #[test]
    fn serialize_releases() {
        let now = time::get_time();

        let mut release = Release {
            name: "serde".to_string(),
            version: "0.0.0".to_string(),
            description: Some("serde makes things other things".to_string()),
            target_name: Some("x86_64-pc-windows-msvc".to_string()),
            rustdoc_status: true,
            release_time: now,
            stars: 100,
        };

        let correct_json = json!({
            "name": "serde",
            "version": "0.0.0",
            "description": "serde makes things other things",
            "target_name": "x86_64-pc-windows-msvc",
            "rustdoc_status": true,
            "release_time": super::super::duration_to_str(now),
            "release_time_rfc3339": time::at(now).rfc3339().to_string(),
            "stars": 100
        });

        assert_eq!(correct_json, serde_json::to_value(&release).unwrap());

        release.target_name = None;
        let correct_json = json!({
            "name": "serde",
            "version": "0.0.0",
            "description": "serde makes things other things",
            "target_name": null,
            "rustdoc_status": true,
            "release_time": super::super::duration_to_str(now),
            "release_time_rfc3339": time::at(now).rfc3339().to_string(),
            "stars": 100
        });

        assert_eq!(correct_json, serde_json::to_value(&release).unwrap());

        release.description = None;
        let correct_json = json!({
            "name": "serde",
            "version": "0.0.0",
            "description": null,
            "target_name": null,
            "rustdoc_status": true,
            "release_time": super::super::duration_to_str(now),
            "release_time_rfc3339": time::at(now).rfc3339().to_string(),
            "stars": 100
        });

        assert_eq!(correct_json, serde_json::to_value(&release).unwrap());
    }

    #[test]
    fn release_feed() {
        wrapper(|env| {
            let web = env.frontend();
            assert_success("/releases/feed", web)
        })
    }
}
