//! Releases web handlers


use super::{NoCrate, duration_to_str};
use super::page::Page;
use super::pool::Pool;
use iron::prelude::*;
use iron::status;
use router::Router;
use rustc_serialize::json::{Json, ToJson};
use std::collections::BTreeMap;
use time;
use postgres::Connection;


/// Number of release in home page
const RELEASES_IN_HOME: i64 = 15;
/// Releases in /releases page
const RELEASES_IN_RELEASES: i64 = 30;


struct Release {
    name: String,
    version: String,
    description: Option<String>,
    target_name: Option<String>,
    rustdoc_status: bool,
    release_time: time::Timespec,
}


impl ToJson for Release {
    fn to_json(&self) -> Json {
        let mut m: BTreeMap<String, Json> = BTreeMap::new();
        m.insert("name".to_string(), self.name.to_json());
        m.insert("version".to_string(), self.version.to_json());
        m.insert("description".to_string(), self.description.to_json());
        m.insert("target_name".to_string(), self.target_name.to_json());
        m.insert("rustdoc_status".to_string(), self.rustdoc_status.to_json());
        m.insert("release_time".to_string(),
                 duration_to_str(self.release_time).to_json());
        m.to_json()
    }
}


fn get_releases(conn: &Connection, page: i64, limit: i64) -> Vec<Release> {
    let mut packages = Vec::new();

    let offset = (page - 1) * limit;

    for row in &conn.query("SELECT crates.name, \
                                   releases.version, \
                                   releases.description, \
                                   releases.target_name, \
                                   releases.release_time, \
                                   releases.rustdoc_status \
                            FROM crates \
                            INNER JOIN releases ON crates.id = releases.crate_id \
                            ORDER BY releases.release_time DESC \
                            LIMIT $1 OFFSET $2",
                           &[&limit, &offset])
                    .unwrap() {

        let package = Release {
            name: row.get(0),
            version: row.get(1),
            description: row.get(2),
            target_name: row.get(3),
            release_time: row.get(4),
            rustdoc_status: row.get(5),
        };

        packages.push(package);
    }

    packages
}



fn get_search_results(conn: &Connection,
                      query: &str,
                      page: i64,
                      limit: i64)
                      -> Option<(i64, Vec<Release>)> {

    let offset = (page - 1) * limit;
    let mut packages = Vec::new();

    for row in &conn.query("SELECT crates.name, \
                                   releases.version, \
                                   releases.description, \
                                   releases.target_name, \
                                   releases.release_time, \
                                   releases.rustdoc_status, \
                                   ts_rank_cd(crates.content, to_tsquery($1)) AS rank \
                            FROM crates \
                            INNER JOIN releases ON crates.latest_version_id = releases.id \
                            WHERE crates.content @@ to_tsquery($1) \
                            ORDER BY rank DESC \
                            LIMIT $2 OFFSET $3",
                           &[&query, &limit, &offset])
                    .unwrap() {

        let package = Release {
            name: row.get(0),
            version: row.get(1),
            description: row.get(2),
            target_name: row.get(3),
            release_time: row.get(4),
            rustdoc_status: row.get(5),
        };

        packages.push(package);
    }

    if !packages.is_empty() {
        // get count of total results
        let rows = conn.query("SELECT COUNT(*) FROM crates WHERE content @@ to_tsquery($1)",
                              &[&query])
                       .unwrap();

        Some((rows.get(0).get(0), packages))
    } else {
        None
    }
}



pub fn home_page(req: &mut Request) -> IronResult<Response> {
    let conn = req.extensions.get::<Pool>().unwrap();
    let packages = get_releases(conn, 1, RELEASES_IN_HOME);
    Page::new(packages)
        .set_true("show_search_form")
        .set_true("hide_package_navigation")
        .to_resp("releases")
}


pub fn releases_handler(req: &mut Request) -> IronResult<Response> {

    // page number of releases
    let page_number: i64 = req.extensions
                              .get::<Router>()
                              .unwrap()
                              .find("page")
                              .unwrap_or("1")
                              .parse()
                              .unwrap_or(1);

    let conn = req.extensions.get::<Pool>().unwrap();
    let packages = get_releases(conn, page_number, RELEASES_IN_RELEASES);
    let page = {
        let page = Page::new(packages)
                       .title("Recent Releases")
                       .set_int("next_page", page_number + 1);

        // Set previous page if we are not in first page
        // TODO: Currently, there is no way to know we are on the last page.
        //       TBH I kinda don't care. COUNT(*) is expensive, and there is more than
        //       25k release anyway, I don't think people will check last page. I can cache
        //       result and use this value for approximation. But since I don't know how to
        //       do it yet, I will just skip page checking. I can also assume if Package count
        //       is less than RELEASES_IN_RELEASES, we are on last page.
        if page_number == 1 {
            page
        } else {
            page.set_int("previous_page", page_number - 1)
        }
    };


    page.set_int("next_page", page_number + 1).to_resp("releases")
}



pub fn search_handler(req: &mut Request) -> IronResult<Response> {
    use params::{Params, Value};

    let params = req.get::<Params>().unwrap();
    let query = params.find(&["query"]);

    let conn = req.extensions.get::<Pool>().unwrap();
    if let Some(&Value::String(ref query)) = query {
        let search_query = query.replace(" ", " & ");
        get_search_results(&conn, &search_query, 1, RELEASES_IN_RELEASES)
            .ok_or(IronError::new(NoCrate, status::NotFound))
            .and_then(|(_, results)| {
                // FIXME: There is no pagination
                Page::new(results)
                    .set("search_query", &query)
                    .title(&format!("Search results for '{}'", query))
                    .to_resp("releases")
            })
    } else {
        Err(IronError::new(NoCrate, status::NotFound))
    }
}
