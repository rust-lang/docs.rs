//! Releases web handlers

use super::error::Nope;
use super::page::Page;
use super::pool::Pool;
use super::{duration_to_str, match_version, redirect_base};
use iron::prelude::*;
use iron::status;
use postgres::Connection;
use router::Router;
use rustc_serialize::json::{Json, ToJson};
use std::collections::BTreeMap;

/// Number of release in home page
const RELEASES_IN_HOME: i64 = 15;
/// Releases in /releases page
const RELEASES_IN_RELEASES: i64 = 30;
/// Releases in recent releases feed
const RELEASES_IN_FEED: i64 = 150;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Release {
    name: String,
    version: String,
    description: Option<String>,
    target_name: Option<String>,
    rustdoc_status: bool,
    release_time: time::Timespec,
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

impl ToJson for Release {
    fn to_json(&self) -> Json {
        let mut m: BTreeMap<String, Json> = BTreeMap::new();
        m.insert("name".to_string(), self.name.to_json());
        m.insert("version".to_string(), self.version.to_json());
        m.insert("description".to_string(), self.description.to_json());
        m.insert("target_name".to_string(), self.target_name.to_json());
        m.insert("rustdoc_status".to_string(), self.rustdoc_status.to_json());
        m.insert(
            "release_time".to_string(),
            duration_to_str(self.release_time).to_json(),
        );
        m.insert(
            "release_time_rfc3339".to_string(),
            format!("{}", time::at(self.release_time).rfc3339()).to_json(),
        );
        m.insert("stars".to_string(), self.stars.to_json());
        m.to_json()
    }
}

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
enum Order {
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

fn get_releases(conn: &Connection, page: i64, limit: i64, order: Order) -> Vec<Release> {
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

    let mut packages = Vec::new();
    for row in &conn.query(&query, &[&limit, &offset]).unwrap() {
        let package = Release {
            name: row.get(0),
            version: row.get(1),
            description: row.get(2),
            target_name: row.get(3),
            release_time: row.get(4),
            rustdoc_status: row.get(5),
            stars: row.get(6),
        };

        packages.push(package);
    }

    packages
}

fn get_releases_by_author(
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
                        authors.name
                 FROM crates
                 INNER JOIN releases ON releases.id = crates.latest_version_id
                 INNER JOIN author_rels ON releases.id = author_rels.rid
                 INNER JOIN authors ON authors.id = author_rels.aid
                 WHERE authors.slug = $1
                 ORDER BY crates.github_stars DESC
                 LIMIT $2 OFFSET $3";

    let mut author_name = String::new();
    let mut packages = Vec::new();
    for row in &conn.query(&query, &[&author, &limit, &offset]).unwrap() {
        let package = Release {
            name: row.get(0),
            version: row.get(1),
            description: row.get(2),
            target_name: row.get(3),
            release_time: row.get(4),
            rustdoc_status: row.get(5),
            stars: row.get(6),
        };

        author_name = row.get(7);
        packages.push(package);
    }

    (author_name, packages)
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

    let mut author_name = String::new();
    let mut packages = Vec::new();
    for row in &conn.query(&query, &[&author, &limit, &offset]).unwrap() {
        let package = Release {
            name: row.get(0),
            version: row.get(1),
            description: row.get(2),
            target_name: row.get(3),
            release_time: row.get(4),
            rustdoc_status: row.get(5),
            stars: row.get(6),
        };

        author_name = if !row.get::<usize, String>(7).is_empty() {
            row.get(7)
        } else {
            row.get(8)
        };
        packages.push(package);
    }

    (author_name, packages)
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
    let conn = extension!(req, Pool).get();
    let packages = get_releases(&conn, 1, RELEASES_IN_HOME, Order::ReleaseTime);
    Page::new(packages)
        .set_true("show_search_form")
        .set_true("hide_package_navigation")
        .to_resp("releases")
}

pub fn releases_feed_handler(req: &mut Request) -> IronResult<Response> {
    let conn = extension!(req, Pool).get();
    let packages = get_releases(&conn, 1, RELEASES_IN_FEED, Order::ReleaseTime);
    let mut resp = ctry!(Page::new(packages).to_resp("releases_feed"));
    resp.headers.set(::iron::headers::ContentType(
        "application/atom+xml".parse().unwrap(),
    ));
    Ok(resp)
}

fn releases_handler(
    packages: Vec<Release>,
    page_number: i64,
    release_type: &str,
    tab: &str,
    title: &str,
) -> IronResult<Response> {
    if packages.is_empty() {
        return Err(IronError::new(Nope::CrateNotFound, status::NotFound));
    }

    // Show next and previous page buttons
    // This is a temporary solution to avoid expensive COUNT(*)
    let (show_next_page, show_previous_page) = (
        packages.len() == RELEASES_IN_RELEASES as usize,
        page_number != 1,
    );

    Page::new(packages)
        .title("Releases")
        .set("description", title)
        .set("release_type", release_type)
        .set_true("show_releases_navigation")
        .set_true(tab)
        .set_bool("show_next_page_button", show_next_page)
        .set_int("next_page", page_number + 1)
        .set_bool("show_previous_page_button", show_previous_page)
        .set_int("previous_page", page_number - 1)
        .to_resp("releases")
}

// Following functions caused a code repeat due to design of our /releases/ URL routes
pub fn recent_releases_handler(req: &mut Request) -> IronResult<Response> {
    let page_number: i64 = extension!(req, Router)
        .find("page")
        .unwrap_or("1")
        .parse()
        .unwrap_or(1);
    let conn = extension!(req, Pool).get();
    let packages = get_releases(&conn, page_number, RELEASES_IN_RELEASES, Order::ReleaseTime);
    releases_handler(
        packages,
        page_number,
        "recent",
        "releases_navigation_recent_tab",
        "Recently uploaded crates",
    )
}

pub fn releases_by_stars_handler(req: &mut Request) -> IronResult<Response> {
    let page_number: i64 = extension!(req, Router)
        .find("page")
        .unwrap_or("1")
        .parse()
        .unwrap_or(1);
    let conn = extension!(req, Pool).get();
    let packages = get_releases(&conn, page_number, RELEASES_IN_RELEASES, Order::GithubStars);
    releases_handler(
        packages,
        page_number,
        "stars",
        "releases_navigation_stars_tab",
        "Crates with most stars",
    )
}

pub fn releases_recent_failures_handler(req: &mut Request) -> IronResult<Response> {
    let page_number: i64 = extension!(req, Router)
        .find("page")
        .unwrap_or("1")
        .parse()
        .unwrap_or(1);
    let conn = extension!(req, Pool).get();
    let packages = get_releases(
        &conn,
        page_number,
        RELEASES_IN_RELEASES,
        Order::RecentFailures,
    );
    releases_handler(
        packages,
        page_number,
        "recent-failures",
        "releases_navigation_recent_failures_tab",
        "Recent crates failed to build",
    )
}

pub fn releases_failures_by_stars_handler(req: &mut Request) -> IronResult<Response> {
    let page_number: i64 = extension!(req, Router)
        .find("page")
        .unwrap_or("1")
        .parse()
        .unwrap_or(1);
    let conn = extension!(req, Pool).get();
    let packages = get_releases(
        &conn,
        page_number,
        RELEASES_IN_RELEASES,
        Order::FailuresByGithubStars,
    );
    releases_handler(
        packages,
        page_number,
        "failures",
        "releases_navigation_failures_by_stars_tab",
        "Crates with most stars failed to build",
    )
}

pub fn author_handler(req: &mut Request) -> IronResult<Response> {
    let router = extension!(req, Router);
    // page number of releases
    let page_number: i64 = router.find("page").unwrap_or("1").parse().unwrap_or(1);

    let conn = extension!(req, Pool).get();

    #[allow(clippy::or_fun_call)]
    let author = ctry!(router
        .find("author")
        .ok_or(IronError::new(Nope::CrateNotFound, status::NotFound)));

    let (author_name, packages) = if author.starts_with('@') {
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

    if packages.is_empty() {
        return Err(IronError::new(Nope::CrateNotFound, status::NotFound));
    }

    // Show next and previous page buttons
    // This is a temporary solution to avoid expensive COUNT(*)
    let (show_next_page, show_previous_page) = (
        packages.len() == RELEASES_IN_RELEASES as usize,
        page_number != 1,
    );
    Page::new(packages)
        .title("Releases")
        .set("description", &format!("Crates from {}", author_name))
        .set("author", &author_name)
        .set("release_type", author)
        .set_true("show_releases_navigation")
        .set_true("show_stars")
        .set_bool("show_next_page_button", show_next_page)
        .set_int("next_page", page_number + 1)
        .set_bool("show_previous_page_button", show_previous_page)
        .set_int("previous_page", page_number - 1)
        .to_resp("releases")
}

pub fn search_handler(req: &mut Request) -> IronResult<Response> {
    use params::{Params, Value};

    let params = ctry!(req.get::<Params>());
    let query = params.find(&["query"]);

    let conn = extension!(req, Pool).get();
    if let Some(&Value::String(ref query)) = query {
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
        Page::new(results)
            .set("search_query", &query)
            .title(&title)
            .to_resp("releases")
    } else {
        Err(IronError::new(Nope::NoResults, status::NotFound))
    }
}

pub fn activity_handler(req: &mut Request) -> IronResult<Response> {
    let conn = extension!(req, Pool).get();
    let release_activity_data: Json = ctry!(conn.query(
        "SELECT value FROM config WHERE name = 'release_activity'",
        &[]
    ))
    .get(0)
    .get(0);
    Page::new(release_activity_data)
        .title("Releases")
        .set("description", "Monthly release activity")
        .set_true("show_releases_navigation")
        .set_true("releases_navigation_activity_tab")
        .set_true("javascript_highchartjs")
        .to_resp("releases_activity")
}

pub fn build_queue_handler(req: &mut Request) -> IronResult<Response> {
    let conn = extension!(req, Pool).get();
    let mut crates: Vec<(String, String, i32)> = Vec::new();
    for krate in &conn
        .query(
            "SELECT name, version, priority
                              FROM queue
                              WHERE attempt < 5
                              ORDER BY priority ASC, attempt ASC, id ASC",
            &[],
        )
        .unwrap()
    {
        crates.push((
            krate.get("name"),
            krate.get("version"),
            // The priority here is inverted: in the database if a crate has a higher priority it
            // will be built after everything else, which is counter-intuitive for people not
            // familiar with docs.rs's inner workings.
            -krate.get::<_, i32>("priority"),
        ));
    }
    let is_empty = crates.is_empty();
    Page::new(crates)
        .title("Build queue")
        .set("description", "List of crates scheduled to build")
        .set_bool("queue_empty", is_empty)
        .set_true("show_releases_navigation")
        .set_true("releases_queue_tab")
        .to_resp("releases_queue")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test::wrapper;

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
                let (num_results, mut results) =
                    dbg!(get_search_results(&db.conn(), *name, 1, 100));
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
}
