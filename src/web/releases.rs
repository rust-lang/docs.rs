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
use time;

/// Number of release in home page
const RELEASES_IN_HOME: i64 = 15;
/// Releases in /releases page
const RELEASES_IN_RELEASES: i64 = 30;
/// Releases in recent releases feed
const RELEASES_IN_FEED: i64 = 150;

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

enum Order {
    ReleaseTime, // this is default order
    GithubStars,
    RecentFailures,
    FailuresByGithubStars,
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

fn get_search_results(
    conn: &Connection,
    query: &str,
    page: i64,
    limit: i64,
) -> Option<(i64, Vec<Release>)> {
    let offset = (page - 1) * limit;

    let rows = match conn.query(
        "SELECT crates.name,
                                    releases.version,
                                    releases.description,
                                    releases.target_name,
                                    releases.release_time,
                                    releases.rustdoc_status,
                                    ts_rank_cd(crates.content, to_tsquery($1)) AS rank
                                 FROM crates
                                 INNER JOIN releases ON crates.latest_version_id = releases.id
                                 WHERE crates.name LIKE concat('%', $1, '%')
                                    OR crates.content @@ to_tsquery($1)
                                 ORDER BY crates.name = $1 DESC,
                                    crates.name LIKE concat('%', $1, '%') DESC,
                                    rank DESC
                                 LIMIT $2 OFFSET $3",
        &[&query, &limit, &offset],
    ) {
        Ok(r) => r,
        Err(_) => return None,
    };

    let packages: Vec<Release> = rows
        .into_iter()
        .map(|row| Release {
            name: row.get(0),
            version: row.get(1),
            description: row.get(2),
            target_name: row.get(3),
            release_time: row.get(4),
            rustdoc_status: row.get(5),
            ..Release::default()
        })
        .collect();

    if !packages.is_empty() {
        // get count of total results
        let rows = conn
            .query(
                "SELECT COUNT(*) FROM crates WHERE content @@ to_tsquery($1)",
                &[&query],
            )
            .unwrap();

        Some((rows.get(0).get(0), packages))
    } else {
        None
    }
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
            if let Some((version, id)) = match_version(&conn, &query, None).into_option() {
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

        let search_query = query.replace(" ", " & ");
        #[allow(clippy::or_fun_call)]
        get_search_results(&conn, &search_query, 1, RELEASES_IN_RELEASES)
            .ok_or(IronError::new(Nope::NoResults, status::NotFound))
            .and_then(|(_, results)| {
                // FIXME: There is no pagination
                Page::new(results)
                    .set("search_query", &query)
                    .title(&format!("Search results for '{}'", query))
                    .to_resp("releases")
            })
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
    let query = conn
        .query(
            "SELECT name, version, priority
                          FROM queue
                          WHERE attempt < 5
                          ORDER BY priority ASC, attempt ASC, id ASC",
            &[],
        )
        .unwrap();

    let crates: Vec<(String, String, i32)> = query
        .into_iter()
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

    let is_empty = crates.is_empty();
    Page::new(crates)
        .title("Build queue")
        .set("description", "List of crates scheduled to build")
        .set_bool("queue_empty", is_empty)
        .set_true("show_releases_navigation")
        .set_true("releases_queue_tab")
        .to_resp("releases_queue")
}
