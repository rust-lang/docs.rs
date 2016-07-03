


use super::{NoCrate, MetaData, duration_to_str, match_version, render_markdown};
use super::page::Page;
use db::connect_db;
use iron::prelude::*;
use iron::status;
use std::collections::BTreeMap;
use time;
use rustc_serialize::json::{Json, ToJson};
use router::Router;
use postgres::Connection;
use semver;


// TODO: Add target name and versions


#[derive(Debug)]
struct CrateDetails {
    name: String,
    version: String,
    description: Option<String>,
    authors: Vec<(String, String)>,
    authors_json: Option<Json>,
    dependencies: Option<Json>,
    readme: Option<String>,
    rustdoc: Option<String>, // this is description_long in database
    release_time: time::Timespec,
    build_status: bool,
    rustdoc_status: bool,
    repository_url: Option<String>,
    homepage_url: Option<String>,
    keywords: Option<Json>,
    have_examples: bool, // need to check this manually
    target_name: Option<String>,
    versions: Vec<String>,
    github: bool, // is crate hosted in github
    github_stars: Option<i32>,
    github_forks: Option<i32>,
    github_issues: Option<i32>,
    metadata: MetaData,
}


impl ToJson for CrateDetails {
    fn to_json(&self) -> Json {
        let mut m: BTreeMap<String, Json> = BTreeMap::new();
        m.insert("name".to_string(), self.name.to_json());
        m.insert("version".to_string(), self.version.to_json());
        m.insert("description".to_string(), self.description.to_json());
        m.insert("authors".to_string(), self.authors.to_json());
        m.insert("authors_json".to_string(), self.authors_json.to_json());
        m.insert("dependencies".to_string(), self.dependencies.to_json());
        if let Some(ref readme) = self.readme {
            m.insert("readme".to_string(), render_markdown(&readme).to_json());
        }
        if let Some(ref rustdoc) = self.rustdoc {
            m.insert("rustdoc".to_string(), render_markdown(&rustdoc).to_json());
        }
        m.insert("release_time".to_string(),
                 duration_to_str(self.release_time).to_json());
        m.insert("build_status".to_string(), self.build_status.to_json());
        m.insert("rustdoc_status".to_string(), self.rustdoc_status.to_json());
        m.insert("repository_url".to_string(), self.repository_url.to_json());
        m.insert("homepage_url".to_string(), self.homepage_url.to_json());
        m.insert("keywords".to_string(), self.keywords.to_json());
        m.insert("have_examples".to_string(), self.have_examples.to_json());
        m.insert("target_name".to_string(), self.target_name.to_json());
        m.insert("versions".to_string(), self.versions.to_json());
        m.insert("github".to_string(), self.github.to_json());
        m.insert("github_stars".to_string(), self.github_stars.to_json());
        m.insert("github_forks".to_string(), self.github_forks.to_json());
        m.insert("github_issues".to_string(), self.github_issues.to_json());
        m.insert("metadata".to_string(), self.metadata.to_json());
        m.to_json()
    }
}


impl CrateDetails {
    fn new(conn: &Connection, name: &str, version: &str) -> Option<CrateDetails> {

        // get all stuff, I love you rustfmt
        let query = "SELECT crates.name, \
                            releases.version, \
                            releases.description, \
                            releases.authors, \
                            releases.dependencies, \
                            releases.readme, \
                            releases.description_long, \
                            releases.release_time, \
                            releases.build_status, \
                            releases.rustdoc_status, \
                            releases.repository_url, \
                            releases.homepage_url, \
                            releases.keywords, \
                            releases.have_examples, \
                            releases.target_name, \
                            crates.versions, \
                            authors.name, \
                            authors.slug, \
                            crates.github_stars, \
                            crates.github_forks, \
                            crates.github_issues \
                     FROM author_rels \
                     LEFT OUTER JOIN authors ON authors.id = author_rels.aid \
                     LEFT OUTER JOIN releases ON releases.id = author_rels.rid \
                     LEFT OUTER JOIN crates ON crates.id = releases.crate_id \
                     WHERE crates.name = $1 AND releases.version = $2;";

        let rows = conn.query(query, &[&name, &version]).unwrap();

        if rows.len() == 0 {
            return None;
        }

        // sort versions with semver
        let versions = {
            let mut versions: Vec<semver::Version> = Vec::new();
            let versions_from_db: Json = rows.get(0).get(15);

            versions_from_db.as_array().map(|vers| {
                for version in vers {
                    version.as_string().map(|version| {
                        if let Ok(sem_ver) = semver::Version::parse(&version) {
                            versions.push(sem_ver);
                        };
                    });
                }
            });

            versions.sort();
            versions.reverse();
            versions.iter().map(|semver| format!("{}", semver)).collect()
        };

        let metadata = MetaData {
            name: rows.get(0).get(0),
            version: rows.get(0).get(1),
            description: rows.get(0).get(2),
            rustdoc_status: rows.get(0).get(9),
            target_name: rows.get(0).get(14),
        };

        let mut crate_details = CrateDetails {
            name: rows.get(0).get(0),
            version: rows.get(0).get(1),
            description: rows.get(0).get(2),
            authors: Vec::new(),
            authors_json: rows.get(0).get(3),
            dependencies: rows.get(0).get(4),
            readme: rows.get(0).get(5),
            rustdoc: rows.get(0).get(6),
            release_time: rows.get(0).get(7),
            build_status: rows.get(0).get(8),
            rustdoc_status: rows.get(0).get(9),
            repository_url: rows.get(0).get(10),
            homepage_url: rows.get(0).get(11),
            keywords: rows.get(0).get(12),
            have_examples: rows.get(0).get(13),
            target_name: rows.get(0).get(14),
            versions: versions,
            github: false,
            github_stars: rows.get(0).get(18),
            github_forks: rows.get(0).get(19),
            github_issues: rows.get(0).get(20),
            metadata: metadata,
        };

        if let Some(repository_url) = crate_details.repository_url.clone() {
            crate_details.github = repository_url.starts_with("http://github.com") ||
                                   repository_url.starts_with("https://github.com");
        }

        // Insert authors with name and slug
        for row in &rows {
            crate_details.authors.push((row.get(16), row.get(17)));
        }

        Some(crate_details)
    }
}



pub fn crate_details_handler(req: &mut Request) -> IronResult<Response> {
    // this handler must always called with a crate name
    let name = req.extensions.get::<Router>().unwrap().find("name").unwrap();
    let req_version = req.extensions.get::<Router>().unwrap().find("version");

    let conn = connect_db().unwrap();

    match_version(&conn, &name, req_version)
        .and_then(|version| CrateDetails::new(&conn, &name, &version))
        .ok_or(IronError::new(NoCrate, status::NotFound))
        .and_then(|details| {
            Page::new(details)
                .set_true("show_package_navigation")
                .set_true("javascript_highlightjs")
                .set_true("package_navigation_crate_tab")
                .to_resp("crate_details")
        })
}
