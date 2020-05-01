use super::error::Nope;
use super::page::Page;
use super::pool::Pool;
use super::{
    duration_to_str, match_version, redirect_base, render_markdown, MatchSemver, MetaData,
};
use iron::prelude::*;
use iron::{status, Url};
use postgres::Connection;
use router::Router;
use rustc_serialize::json::{Json, ToJson};
use std::collections::BTreeMap;

// TODO: Add target name and versions

#[derive(Debug)]
pub struct CrateDetails {
    name: String,
    version: String,
    description: Option<String>,
    authors: Vec<(String, String)>,
    owners: Vec<(String, String)>,
    authors_json: Option<Json>,
    dependencies: Option<Json>,
    readme: Option<String>,
    rustdoc: Option<String>, // this is description_long in database
    release_time: time::Timespec,
    build_status: bool,
    last_successful_build: Option<String>,
    rustdoc_status: bool,
    repository_url: Option<String>,
    homepage_url: Option<String>,
    keywords: Option<Json>,
    have_examples: bool, // need to check this manually
    pub target_name: String,
    releases: Vec<Release>,
    github: bool, // is crate hosted in github
    github_stars: Option<i32>,
    github_forks: Option<i32>,
    github_issues: Option<i32>,
    pub(crate) metadata: MetaData,
    is_library: bool,
    yanked: bool,
    pub(crate) doc_targets: Vec<String>,
    license: Option<String>,
    documentation_url: Option<String>,
}

impl ToJson for CrateDetails {
    fn to_json(&self) -> Json {
        let mut m: BTreeMap<String, Json> = BTreeMap::new();
        m.insert("name".to_string(), self.name.to_json());
        m.insert("version".to_string(), self.version.to_json());
        m.insert("description".to_string(), self.description.to_json());
        m.insert("authors".to_string(), self.authors.to_json());
        m.insert("owners".to_string(), self.owners.to_json());
        m.insert("authors_json".to_string(), self.authors_json.to_json());
        m.insert("dependencies".to_string(), self.dependencies.to_json());
        if let Some(ref readme) = self.readme {
            m.insert("readme".to_string(), render_markdown(&readme).to_json());
        }
        if let Some(ref rustdoc) = self.rustdoc {
            m.insert("rustdoc".to_string(), render_markdown(&rustdoc).to_json());
        }
        m.insert(
            "release_time".to_string(),
            duration_to_str(self.release_time).to_json(),
        );
        m.insert("build_status".to_string(), self.build_status.to_json());
        m.insert(
            "last_successful_build".to_string(),
            self.last_successful_build.to_json(),
        );
        m.insert("rustdoc_status".to_string(), self.rustdoc_status.to_json());
        m.insert("repository_url".to_string(), self.repository_url.to_json());
        m.insert("homepage_url".to_string(), self.homepage_url.to_json());
        m.insert("keywords".to_string(), self.keywords.to_json());
        m.insert("have_examples".to_string(), self.have_examples.to_json());
        m.insert("target_name".to_string(), self.target_name.to_json());
        m.insert("releases".to_string(), self.releases.to_json());
        m.insert("github".to_string(), self.github.to_json());
        m.insert("github_stars".to_string(), self.github_stars.to_json());
        m.insert("github_forks".to_string(), self.github_forks.to_json());
        m.insert("github_issues".to_string(), self.github_issues.to_json());
        m.insert("metadata".to_string(), self.metadata.to_json());
        m.insert("is_library".to_string(), self.is_library.to_json());
        m.insert("yanked".to_string(), self.yanked.to_json());
        m.insert("doc_targets".to_string(), self.doc_targets.to_json());
        m.insert("license".to_string(), self.license.to_json());
        m.insert(
            "documentation_url".to_string(),
            self.documentation_url.to_json(),
        );
        m.to_json()
    }
}

#[derive(Debug, Eq, PartialEq)]
pub struct Release {
    pub version: String,
    pub build_status: bool,
    pub yanked: bool,
}

impl ToJson for Release {
    fn to_json(&self) -> Json {
        let mut m: BTreeMap<String, Json> = BTreeMap::new();
        m.insert("version".to_string(), self.version.to_json());
        m.insert("build_status".to_string(), self.build_status.to_json());
        m.insert("yanked".to_string(), self.yanked.to_json());
        m.to_json()
    }
}

impl CrateDetails {
    pub fn new(conn: &Connection, name: &str, version: &str) -> Option<CrateDetails> {
        // get all stuff, I love you rustfmt
        let query = "SELECT crates.id,
                releases.id,
                crates.name,
                releases.version,
                releases.description,
                releases.authors,
                releases.dependencies,
                releases.readme,
                releases.description_long,
                releases.release_time,
                releases.build_status,
                releases.rustdoc_status,
                releases.repository_url,
                releases.homepage_url,
                releases.keywords,
                releases.have_examples,
                releases.target_name,
                crates.versions,
                crates.github_stars,
                crates.github_forks,
                crates.github_issues,
                releases.is_library,
                releases.yanked,
                releases.doc_targets,
                releases.license,
                releases.documentation_url,
                releases.default_target
         FROM releases
         INNER JOIN crates ON releases.crate_id = crates.id
         WHERE crates.name = $1 AND releases.version = $2;";

        let rows = conn.query(query, &[&name, &version]).unwrap();

        if rows.is_empty() {
            return None;
        }

        let crate_id: i32 = rows.get(0).get(0);
        let release_id: i32 = rows.get(0).get(1);

        // sort versions with semver
        let releases = {
            let mut versions: Vec<semver::Version> = Vec::new();
            let versions_from_db: Json = rows.get(0).get(17);

            if let Some(vers) = versions_from_db.as_array() {
                for version in vers {
                    if let Some(version) = version.as_string() {
                        if let Ok(sem_ver) = semver::Version::parse(&version) {
                            versions.push(sem_ver);
                        }
                    }
                }
            }

            versions.sort();
            versions.reverse();
            versions
                .iter()
                .map(|version| map_to_release(&conn, crate_id, version.to_string()))
                .collect()
        };

        let metadata = MetaData {
            name: rows.get(0).get(2),
            version: rows.get(0).get(3),
            description: rows.get(0).get(4),
            rustdoc_status: rows.get(0).get(11),
            target_name: rows.get(0).get(16),
            default_target: rows.get(0).get(26),
        };

        let doc_targets = {
            let data: Json = rows.get(0).get(23);
            data.as_array()
                .map(|array| {
                    array
                        .iter()
                        .filter_map(|item| item.as_string().map(|s| s.to_owned()))
                        .collect()
                })
                .unwrap_or_else(Vec::new)
        };

        let mut crate_details = CrateDetails {
            name: rows.get(0).get(2),
            version: rows.get(0).get(3),
            description: rows.get(0).get(4),
            authors: Vec::new(),
            owners: Vec::new(),
            authors_json: rows.get(0).get(5),
            dependencies: rows.get(0).get(6),
            readme: rows.get(0).get(7),
            rustdoc: rows.get(0).get(8),
            release_time: rows.get(0).get(9),
            build_status: rows.get(0).get(10),
            last_successful_build: None,
            rustdoc_status: rows.get(0).get(11),
            repository_url: rows.get(0).get(12),
            homepage_url: rows.get(0).get(13),
            keywords: rows.get(0).get(14),
            have_examples: rows.get(0).get(15),
            target_name: rows.get(0).get(16),
            releases,
            github: false,
            github_stars: rows.get(0).get(18),
            github_forks: rows.get(0).get(19),
            github_issues: rows.get(0).get(20),
            metadata,
            is_library: rows.get(0).get(21),
            yanked: rows.get(0).get(22),
            doc_targets,
            license: rows.get(0).get(24),
            documentation_url: rows.get(0).get(25),
        };

        if let Some(repository_url) = crate_details.repository_url.clone() {
            crate_details.github = repository_url.starts_with("http://github.com")
                || repository_url.starts_with("https://github.com");
        }

        // get authors
        for row in &conn
            .query(
                "SELECT name, slug
                                FROM authors
                                INNER JOIN author_rels ON author_rels.aid = authors.id
                                WHERE rid = $1",
                &[&release_id],
            )
            .unwrap()
        {
            crate_details.authors.push((row.get(0), row.get(1)));
        }

        // get owners
        for row in &conn
            .query(
                "SELECT login, avatar
                                FROM owners
                                INNER JOIN owner_rels ON owner_rels.oid = owners.id
                                WHERE cid = $1",
                &[&crate_id],
            )
            .unwrap()
        {
            crate_details.owners.push((row.get(0), row.get(1)));
        }

        if !crate_details.build_status {
            crate_details.last_successful_build = crate_details
                .releases
                .iter()
                .filter(|release| release.build_status && !release.yanked)
                .map(|release| release.version.to_owned())
                .next();
        }

        Some(crate_details)
    }

    /// Returns the latest non-yanked release of this crate (or latest yanked if they are all
    /// yanked).
    pub fn latest_release(&self) -> &Release {
        self.releases
            .iter()
            .find(|release| !release.yanked)
            .unwrap_or(&self.releases[0])
    }
}

fn map_to_release(conn: &Connection, crate_id: i32, version: String) -> Release {
    let rows = conn
        .query(
            "SELECT build_status, yanked
         FROM releases
         WHERE releases.crate_id = $1 and releases.version = $2;",
            &[&crate_id, &version],
        )
        .unwrap();

    let (build_status, yanked) = if !rows.is_empty() {
        (rows.get(0).get(0), rows.get(0).get(1))
    } else {
        Default::default()
    };

    Release {
        version,
        build_status,
        yanked,
    }
}

pub fn crate_details_handler(req: &mut Request) -> IronResult<Response> {
    let router = extension!(req, Router);
    // this handler must always called with a crate name
    let name = cexpect!(router.find("name"));
    let req_version = router.find("version");

    let conn = extension!(req, Pool).get();

    match match_version(&conn, &name, req_version).and_then(|m| m.assume_exact()) {
        Some(MatchSemver::Exact((version, _))) => {
            let details = CrateDetails::new(&conn, &name, &version);

            Page::new(details)
                .set_true("show_package_navigation")
                .set_true("javascript_highlightjs")
                .set_true("package_navigation_crate_tab")
                .to_resp("crate_details")
        }
        Some(MatchSemver::Semver((version, _))) => {
            let url = ctry!(Url::parse(
                &format!("{}/crate/{}/{}", redirect_base(req), name, version)[..]
            ));

            Ok(super::redirect(url))
        }
        None => Err(IronError::new(Nope::CrateNotFound, status::NotFound)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test::TestDatabase;
    use failure::Error;

    fn assert_last_successful_build_equals(
        db: &TestDatabase,
        package: &str,
        version: &str,
        expected_last_successful_build: Option<&str>,
    ) -> Result<(), Error> {
        let details = CrateDetails::new(&db.conn(), package, version)
            .ok_or(failure::err_msg("could not fetch crate details"))?;

        assert_eq!(
            details.last_successful_build,
            expected_last_successful_build.map(|s| s.to_string()),
        );
        Ok(())
    }

    #[test]
    fn test_last_successful_build_when_last_releases_failed_or_yanked() {
        crate::test::wrapper(|env| {
            let db = env.db();

            db.fake_release().name("foo").version("0.0.1").create()?;
            db.fake_release().name("foo").version("0.0.2").create()?;
            db.fake_release()
                .name("foo")
                .version("0.0.3")
                .build_result_successful(false)
                .create()?;
            db.fake_release()
                .name("foo")
                .version("0.0.4")
                .cratesio_data_yanked(true)
                .create()?;
            db.fake_release()
                .name("foo")
                .version("0.0.5")
                .build_result_successful(false)
                .cratesio_data_yanked(true)
                .create()?;

            assert_last_successful_build_equals(&db, "foo", "0.0.1", None)?;
            assert_last_successful_build_equals(&db, "foo", "0.0.2", None)?;
            assert_last_successful_build_equals(&db, "foo", "0.0.3", Some("0.0.2"))?;
            assert_last_successful_build_equals(&db, "foo", "0.0.4", None)?;
            assert_last_successful_build_equals(&db, "foo", "0.0.5", Some("0.0.2"))?;
            Ok(())
        });
    }

    #[test]
    fn test_last_successful_build_when_all_releases_failed_or_yanked() {
        crate::test::wrapper(|env| {
            let db = env.db();

            db.fake_release()
                .name("foo")
                .version("0.0.1")
                .build_result_successful(false)
                .create()?;
            db.fake_release()
                .name("foo")
                .version("0.0.2")
                .build_result_successful(false)
                .create()?;
            db.fake_release()
                .name("foo")
                .version("0.0.3")
                .cratesio_data_yanked(true)
                .create()?;

            assert_last_successful_build_equals(&db, "foo", "0.0.1", None)?;
            assert_last_successful_build_equals(&db, "foo", "0.0.2", None)?;
            assert_last_successful_build_equals(&db, "foo", "0.0.3", None)?;
            Ok(())
        });
    }

    #[test]
    fn test_last_successful_build_with_intermittent_releases_failed_or_yanked() {
        crate::test::wrapper(|env| {
            let db = env.db();

            db.fake_release().name("foo").version("0.0.1").create()?;
            db.fake_release()
                .name("foo")
                .version("0.0.2")
                .build_result_successful(false)
                .create()?;
            db.fake_release()
                .name("foo")
                .version("0.0.3")
                .cratesio_data_yanked(true)
                .create()?;
            db.fake_release().name("foo").version("0.0.4").create()?;

            assert_last_successful_build_equals(&db, "foo", "0.0.1", None)?;
            assert_last_successful_build_equals(&db, "foo", "0.0.2", Some("0.0.4"))?;
            assert_last_successful_build_equals(&db, "foo", "0.0.3", None)?;
            assert_last_successful_build_equals(&db, "foo", "0.0.4", None)?;
            Ok(())
        });
    }

    #[test]
    fn test_releases_should_be_sorted() {
        crate::test::wrapper(|env| {
            let db = env.db();

            // Add new releases of 'foo' out-of-order since CrateDetails should sort them descending
            db.fake_release().name("foo").version("0.1.0").create()?;
            db.fake_release().name("foo").version("0.1.1").create()?;
            db.fake_release()
                .name("foo")
                .version("0.3.0")
                .build_result_successful(false)
                .create()?;
            db.fake_release().name("foo").version("1.0.0").create()?;
            db.fake_release().name("foo").version("0.12.0").create()?;
            db.fake_release()
                .name("foo")
                .version("0.2.0")
                .cratesio_data_yanked(true)
                .create()?;
            db.fake_release()
                .name("foo")
                .version("0.2.0-alpha")
                .create()?;

            let details = CrateDetails::new(&db.conn(), "foo", "0.2.0").unwrap();
            assert_eq!(
                details.releases,
                vec![
                    Release {
                        version: "1.0.0".to_string(),
                        build_status: true,
                        yanked: false
                    },
                    Release {
                        version: "0.12.0".to_string(),
                        build_status: true,
                        yanked: false
                    },
                    Release {
                        version: "0.3.0".to_string(),
                        build_status: false,
                        yanked: false
                    },
                    Release {
                        version: "0.2.0".to_string(),
                        build_status: true,
                        yanked: true
                    },
                    Release {
                        version: "0.2.0-alpha".to_string(),
                        build_status: true,
                        yanked: false
                    },
                    Release {
                        version: "0.1.1".to_string(),
                        build_status: true,
                        yanked: false
                    },
                    Release {
                        version: "0.1.0".to_string(),
                        build_status: true,
                        yanked: false
                    },
                ]
            );

            Ok(())
        });
    }

    #[test]
    fn test_latest_version() {
        crate::test::wrapper(|env| {
            let db = env.db();

            db.fake_release().name("foo").version("0.0.1").create()?;
            db.fake_release().name("foo").version("0.0.3").create()?;
            db.fake_release().name("foo").version("0.0.2").create()?;

            for version in &["0.0.1", "0.0.2", "0.0.3"] {
                let details = CrateDetails::new(&db.conn(), "foo", version).unwrap();
                assert_eq!(details.latest_release().version, "0.0.3");
            }

            Ok(())
        })
    }

    #[test]
    fn test_latest_version_ignores_yanked() {
        crate::test::wrapper(|env| {
            let db = env.db();

            db.fake_release().name("foo").version("0.0.1").create()?;
            db.fake_release()
                .name("foo")
                .version("0.0.3")
                .cratesio_data_yanked(true)
                .create()?;
            db.fake_release().name("foo").version("0.0.2").create()?;

            for version in &["0.0.1", "0.0.2", "0.0.3"] {
                let details = CrateDetails::new(&db.conn(), "foo", version).unwrap();
                assert_eq!(details.latest_release().version, "0.0.2");
            }

            Ok(())
        })
    }

    #[test]
    fn test_latest_version_only_yanked() {
        crate::test::wrapper(|env| {
            let db = env.db();

            db.fake_release()
                .name("foo")
                .version("0.0.1")
                .cratesio_data_yanked(true)
                .create()?;
            db.fake_release()
                .name("foo")
                .version("0.0.3")
                .cratesio_data_yanked(true)
                .create()?;
            db.fake_release()
                .name("foo")
                .version("0.0.2")
                .cratesio_data_yanked(true)
                .create()?;

            for version in &["0.0.1", "0.0.2", "0.0.3"] {
                let details = CrateDetails::new(&db.conn(), "foo", version).unwrap();
                assert_eq!(details.latest_release().version, "0.0.3");
            }

            Ok(())
        })
    }
}
