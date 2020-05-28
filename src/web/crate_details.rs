use super::error::Nope;
use super::page::{CrateDetailsPage, WebPage};
use super::pool::Pool;
use super::{
    duration_to_str, match_version, redirect_base, render_markdown, MatchSemver, MetaData,
};
use iron::prelude::*;
use iron::{status, Url};
use postgres::Connection;
use router::Router;
use serde::{
    ser::{SerializeStruct, Serializer},
    Serialize,
};
use serde_json::Value;

// TODO: Add target name and versions

#[derive(Debug, Clone, PartialEq)]
pub struct CrateDetails {
    name: String,
    version: String,
    description: Option<String>,
    authors: Vec<(String, String)>,
    owners: Vec<(String, String)>,
    authors_json: Option<Value>,
    dependencies: Option<Value>,
    readme: Option<String>,
    rustdoc: Option<String>, // this is description_long in database
    release_time: time::Timespec,
    build_status: bool,
    last_successful_build: Option<String>,
    rustdoc_status: bool,
    repository_url: Option<String>,
    homepage_url: Option<String>,
    keywords: Option<Value>,
    have_examples: bool, // need to check this manually
    pub target_name: String,
    releases: Vec<Release>,
    github: bool, // is crate hosted in github
    github_stars: Option<i32>,
    github_forks: Option<i32>,
    github_issues: Option<i32>,
    // TODO: Is this even needed for rendering pages?
    pub(crate) metadata: MetaData,
    is_library: bool,
    yanked: bool,
    pub(crate) doc_targets: Vec<String>,
    license: Option<String>,
    documentation_url: Option<String>,
}

impl Serialize for CrateDetails {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        // Make sure that the length parameter passed to serde is correct by
        // adding the someness of `readme` and `rustdoc` to the total. `true`
        // is 1 and `false` is 0, so it increments if the value is some (and therefore
        // needs to be serialized)
        let mut state = serializer.serialize_struct(
            "CrateDetails",
            26 + self.readme.is_some() as usize + self.rustdoc.is_some() as usize,
        )?;

        state.serialize_field("metadata", &self.metadata)?;
        state.serialize_field("name", &self.name)?;
        state.serialize_field("version", &self.version)?;
        state.serialize_field("description", &self.description)?;
        state.serialize_field("authors", &self.authors)?;
        state.serialize_field("owners", &self.owners)?;
        state.serialize_field("authors_json", &self.authors_json)?;
        state.serialize_field("dependencies", &self.dependencies)?;

        if let Some(ref readme) = self.readme {
            state.serialize_field("readme", &render_markdown(&readme))?;
        }

        if let Some(ref rustdoc) = self.rustdoc {
            state.serialize_field("rustdoc", &render_markdown(&rustdoc))?;
        }

        state.serialize_field("release_time", &duration_to_str(self.release_time))?;
        state.serialize_field("build_status", &self.build_status)?;
        state.serialize_field("last_successful_build", &self.last_successful_build)?;
        state.serialize_field("rustdoc_status", &self.rustdoc_status)?;
        state.serialize_field("repository_url", &self.repository_url)?;
        state.serialize_field("homepage_url", &self.homepage_url)?;
        state.serialize_field("keywords", &self.keywords)?;
        state.serialize_field("have_examples", &self.have_examples)?;
        state.serialize_field("target_name", &self.target_name)?;
        state.serialize_field("releases", &self.releases)?;
        state.serialize_field("github", &self.github)?;
        state.serialize_field("github_stars", &self.github_stars)?;
        state.serialize_field("github_forks", &self.github_forks)?;
        state.serialize_field("github_issues", &self.github_issues)?;
        state.serialize_field("metadata", &self.metadata)?;
        state.serialize_field("is_library", &self.is_library)?;
        state.serialize_field("doc_targets", &self.doc_targets)?;
        state.serialize_field("yanked", &self.yanked)?;
        state.serialize_field("license", &self.license)?;
        state.serialize_field("documentation_url", &self.documentation_url)?;

        state.end()
    }
}

#[derive(Debug, Clone, Eq, PartialEq, Serialize)]
pub struct Release {
    pub version: String,
    pub build_status: bool,
    pub yanked: bool,
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
            let versions_from_db: Value = rows.get(0).get(17);

            if let Some(versions_from_db) = versions_from_db.as_array() {
                let mut versions: Vec<semver::Version> = versions_from_db
                    .iter()
                    .filter_map(|version| {
                        if let Some(version) = version.as_str() {
                            if let Ok(sem_ver) = semver::Version::parse(&version) {
                                return Some(sem_ver);
                            }
                        }

                        None
                    })
                    .collect();

                versions.sort();
                versions.reverse();
                versions
                    .iter()
                    .map(|version| map_to_release(&conn, crate_id, version.to_string()))
                    .collect()
            } else {
                Vec::new()
            }
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
            let data: Value = rows.get(0).get(23);
            data.as_array()
                .map(|array| {
                    array
                        .iter()
                        .filter_map(|item| item.as_str().map(|s| s.to_owned()))
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
        let authors = conn
            .query(
                "SELECT name, slug
                 FROM authors
                 INNER JOIN author_rels ON author_rels.aid = authors.id
                 WHERE rid = $1",
                &[&release_id],
            )
            .unwrap();

        crate_details.authors = authors
            .into_iter()
            .map(|row| (row.get(0), row.get(1)))
            .collect();

        // get owners
        let owners = conn
            .query(
                "SELECT login, avatar
                 FROM owners
                 INNER JOIN owner_rels ON owner_rels.oid = owners.id
                 WHERE cid = $1",
                &[&crate_id],
            )
            .unwrap();

        crate_details.owners = owners
            .into_iter()
            .map(|row| (row.get(0), row.get(1)))
            .collect();

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

    #[cfg(test)]
    pub fn default_tester(release_time: time::Timespec) -> Self {
        Self {
            name: "rcc".to_string(),
            version: "100.0.0".to_string(),
            description: None,
            authors: vec![],
            owners: vec![],
            authors_json: None,
            dependencies: None,
            readme: None,
            rustdoc: None,
            release_time,
            build_status: true,
            last_successful_build: None,
            rustdoc_status: true,
            repository_url: None,
            homepage_url: None,
            keywords: None,
            yanked: false,
            have_examples: true,
            target_name: "x86_64-unknown-linux-gnu".to_string(),
            releases: vec![],
            github: true,
            github_stars: None,
            github_forks: None,
            github_issues: None,
            metadata: MetaData {
                name: "serde".to_string(),
                version: "1.0.0".to_string(),
                description: Some("serde does stuff".to_string()),
                target_name: None,
                rustdoc_status: true,
                default_target: "x86_64-unknown-linux-gnu".to_string(),
            },
            is_library: true,
            doc_targets: vec![],
            license: None,
            documentation_url: None,
        }
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

    let conn = extension!(req, Pool).get()?;

    match match_version(&conn, &name, req_version).and_then(|m| m.assume_exact()) {
        Some(MatchSemver::Exact((version, _))) => {
            let details = CrateDetails::new(&conn, &name, &version);

            CrateDetailsPage { details }.into_response()
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
    use serde_json::json;

    fn assert_last_successful_build_equals(
        db: &TestDatabase,
        package: &str,
        version: &str,
        expected_last_successful_build: Option<&str>,
    ) -> Result<(), Error> {
        let details = CrateDetails::new(&db.conn(), package, version)
            .ok_or_else(|| failure::err_msg("could not fetch crate details"))?;

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
                .yanked(true)
                .create()?;
            db.fake_release()
                .name("foo")
                .version("0.0.5")
                .build_result_successful(false)
                .yanked(true)
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
                .yanked(true)
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
                .yanked(true)
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
                .yanked(true)
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
                .yanked(true)
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
                .yanked(true)
                .create()?;
            db.fake_release()
                .name("foo")
                .version("0.0.3")
                .yanked(true)
                .create()?;
            db.fake_release()
                .name("foo")
                .version("0.0.2")
                .yanked(true)
                .create()?;

            for version in &["0.0.1", "0.0.2", "0.0.3"] {
                let details = CrateDetails::new(&db.conn(), "foo", version).unwrap();
                assert_eq!(details.latest_release().version, "0.0.3");
            }

            Ok(())
        })
    }

    #[test]
    fn serialize_crate_details() {
        let time = time::get_time();
        let mut details = CrateDetails::default_tester(time);

        let mut correct_json = json!({
            "name": "rcc",
            "version": "100.0.0",
            "description": null,
            "authors": [],
            "owners": [],
            "authors_json": null,
            "dependencies": null,
            "release_time": super::super::duration_to_str(time),
            "build_status": true,
            "last_successful_build": null,
            "rustdoc_status": true,
            "repository_url": null,
            "homepage_url": null,
            "keywords": null,
            "have_examples": true,
            "target_name": "x86_64-unknown-linux-gnu",
            "releases": [],
            "github": true,
            "yanked": false,
            "github_stars": null,
            "github_forks": null,
            "github_issues": null,
            "metadata": {
                "name": "serde",
                "version": "1.0.0",
                "description": "serde does stuff",
                "target_name": null,
                "rustdoc_status": true,
                "default_target": "x86_64-unknown-linux-gnu"
            },
            "is_library": true,
            "doc_targets": [],
            "license": null,
            "documentation_url": null
        });

        assert_eq!(correct_json, serde_json::to_value(&details).unwrap());

        let authors = vec![("Somebody".to_string(), "somebody@somebody.com".to_string())];
        let owners = vec![("Owner".to_string(), "owner@ownsstuff.com".to_string())];
        let description = "serde does stuff".to_string();

        correct_json["description"] = Value::String(description.clone());
        correct_json["owners"] = serde_json::to_value(&owners).unwrap();
        correct_json["authors_json"] = serde_json::to_value(&authors).unwrap();
        correct_json["authors"] = serde_json::to_value(&authors).unwrap();

        details.description = Some(description);
        details.owners = owners;
        details.authors_json = Some(serde_json::to_value(&authors).unwrap());
        details.authors = authors;

        assert_eq!(correct_json, serde_json::to_value(&details).unwrap());
    }

    #[test]
    fn serialize_releases() {
        let release = Release {
            version: "idkman".to_string(),
            build_status: true,
            yanked: true,
        };

        let correct_json = json!({
            "version": "idkman",
            "build_status": true,
            "yanked": true,
        });

        assert_eq!(correct_json, serde_json::to_value(&release).unwrap());
    }
}
