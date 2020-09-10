use super::{match_version, redirect_base, render_markdown, MatchSemver, MetaData};
use crate::{db::Pool, impl_webpage, web::page::WebPage};
use chrono::{DateTime, NaiveDateTime, Utc};
use iron::prelude::*;
use iron::{status, Url};
use postgres::Client;
use router::Router;
use serde::{ser::Serializer, Serialize};
use serde_json::Value;

// TODO: Add target name and versions

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct CrateDetails {
    name: String,
    version: String,
    description: Option<String>,
    authors: Vec<(String, String)>,
    owners: Vec<(String, String)>,
    authors_json: Option<Value>,
    dependencies: Option<Value>,
    #[serde(serialize_with = "optional_markdown")]
    readme: Option<String>,
    #[serde(serialize_with = "optional_markdown")]
    rustdoc: Option<String>, // this is description_long in database
    release_time: DateTime<Utc>,
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
    pub(crate) metadata: MetaData,
    is_library: bool,
    yanked: bool,
    pub(crate) doc_targets: Vec<String>,
    license: Option<String>,
    documentation_url: Option<String>,
    total_items: Option<f32>,
    documented_items: Option<f32>,
}

fn optional_markdown<S>(markdown: &Option<String>, serializer: S) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    if let Some(ref markdown) = markdown {
        Some(render_markdown(&markdown))
    } else {
        None
    }
    .serialize(serializer)
}

#[derive(Debug, Clone, Eq, PartialEq, Serialize)]
pub struct Release {
    pub version: semver::Version,
    pub build_status: bool,
    pub yanked: bool,
    pub is_library: bool,
}

impl CrateDetails {
    pub fn new(conn: &mut Client, name: &str, version: &str) -> Option<CrateDetails> {
        // get all stuff, I love you rustfmt
        let query = "
            SELECT
                crates.id AS crate_id,
                releases.id AS release_id,
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
                ARRAY(SELECT releases.version FROM releases WHERE releases.crate_id = crates.id) AS versions,
                crates.github_stars,
                crates.github_forks,
                crates.github_issues,
                releases.is_library,
                releases.yanked,
                releases.doc_targets,
                releases.license,
                releases.documentation_url,
                releases.default_target,
                doc_coverage.total_items,
                doc_coverage.documented_items
            FROM releases
            INNER JOIN crates ON releases.crate_id = crates.id
            LEFT JOIN doc_coverage ON doc_coverage.release_id = releases.id
            WHERE crates.name = $1 AND releases.version = $2;";

        let rows = conn.query(query, &[&name, &version]).unwrap();

        let krate = if rows.is_empty() {
            return None;
        } else {
            &rows[0]
        };

        let crate_id: i32 = krate.get("crate_id");
        let release_id: i32 = krate.get("release_id");

        // sort versions with semver
        let releases = {
            let versions: Vec<String> = krate.get("versions");
            let mut versions: Vec<semver::Version> = versions
                .iter()
                .filter_map(|version| semver::Version::parse(&version).ok())
                .collect();

            versions.sort();
            versions.reverse();
            versions
                .into_iter()
                .map(|version| map_to_release(conn, crate_id, version))
                .collect()
        };

        let metadata = MetaData {
            name: krate.get("name"),
            version: krate.get("version"),
            description: krate.get("description"),
            rustdoc_status: krate.get("rustdoc_status"),
            target_name: krate.get("target_name"),
            default_target: krate.get("default_target"),
        };

        let doc_targets = {
            let data: Value = krate.get("doc_targets");
            data.as_array()
                .map(|array| {
                    array
                        .iter()
                        .filter_map(|item| item.as_str().map(|s| s.to_owned()))
                        .collect()
                })
                .unwrap_or_else(Vec::new)
        };

        let documented_items: Option<i32> = krate.get("documented_items");
        let total_items: Option<i32> = krate.get("total_items");

        let mut crate_details = CrateDetails {
            name: krate.get("name"),
            version: krate.get("version"),
            description: krate.get("description"),
            authors: Vec::new(),
            owners: Vec::new(),
            authors_json: krate.get("authors"),
            dependencies: krate.get("dependencies"),
            readme: krate.get("readme"),
            rustdoc: krate.get("description_long"),
            release_time: DateTime::from_utc(krate.get::<_, NaiveDateTime>("release_time"), Utc),
            build_status: krate.get("build_status"),
            last_successful_build: None,
            rustdoc_status: krate.get("rustdoc_status"),
            repository_url: krate.get("repository_url"),
            homepage_url: krate.get("homepage_url"),
            keywords: krate.get("keywords"),
            have_examples: krate.get("have_examples"),
            target_name: krate.get("target_name"),
            releases,
            github: false,
            github_stars: krate.get("github_stars"),
            github_forks: krate.get("github_forks"),
            github_issues: krate.get("github_issues"),
            metadata,
            is_library: krate.get("is_library"),
            yanked: krate.get("yanked"),
            doc_targets,
            license: krate.get("license"),
            documentation_url: krate.get("documentation_url"),
            documented_items: documented_items.map(|v| v as f32),
            total_items: total_items.map(|v| v as f32),
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
            .map(|row| (row.get("name"), row.get("slug")))
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
            .map(|row| (row.get("login"), row.get("avatar")))
            .collect();

        if !crate_details.build_status {
            crate_details.last_successful_build = crate_details
                .releases
                .iter()
                .filter(|release| release.build_status && !release.yanked)
                .map(|release| release.version.to_string())
                .next();
        }

        Some(crate_details)
    }

    /// Returns the latest non-yanked, non-prerelease release of this crate (or latest
    /// yanked/prereleased if that is all that exist).
    pub fn latest_release(&self) -> &Release {
        self.releases
            .iter()
            .find(|release| !release.version.is_prerelease() && !release.yanked)
            .unwrap_or(&self.releases[0])
    }
}

fn map_to_release(conn: &mut Client, crate_id: i32, version: semver::Version) -> Release {
    let rows = conn
        .query(
            "SELECT build_status,
                    yanked,
                    is_library
             FROM releases
             WHERE releases.crate_id = $1 and releases.version = $2;",
            &[&crate_id, &version.to_string()],
        )
        .unwrap();

    let (build_status, yanked, is_library) = rows.get(0).map_or_else(Default::default, |row| {
        (
            row.get("build_status"),
            row.get("yanked"),
            row.get("is_library"),
        )
    });

    Release {
        version,
        build_status,
        yanked,
        is_library,
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
struct CrateDetailsPage {
    details: CrateDetails,
}

impl_webpage! {
    CrateDetailsPage = "crate/details.html",
}

pub fn crate_details_handler(req: &mut Request) -> IronResult<Response> {
    let router = extension!(req, Router);
    // this handler must always called with a crate name
    let name = cexpect!(req, router.find("name"));
    let req_version = router.find("version");

    let mut conn = extension!(req, Pool).get()?;

    match match_version(&mut conn, &name, req_version).and_then(|m| m.assume_exact()) {
        Ok(MatchSemver::Exact((version, _))) => {
            let details = cexpect!(req, CrateDetails::new(&mut conn, &name, &version));

            CrateDetailsPage { details }.into_response(req)
        }

        Ok(MatchSemver::Semver((version, _))) => {
            let url = ctry!(
                req,
                Url::parse(&format!(
                    "{}/crate/{}/{}",
                    redirect_base(req),
                    name,
                    version
                )),
            );

            Ok(super::redirect(url))
        }

        Err(err) => Err(IronError::new(err, status::NotFound)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::index::api::CrateOwner;
    use crate::test::{wrapper, TestDatabase};
    use failure::Error;
    use kuchiki::traits::TendrilSink;

    fn assert_last_successful_build_equals(
        db: &TestDatabase,
        package: &str,
        version: &str,
        expected_last_successful_build: Option<&str>,
    ) -> Result<(), Error> {
        let details = CrateDetails::new(&mut db.conn(), package, version)
            .ok_or_else(|| failure::err_msg("could not fetch crate details"))?;

        assert_eq!(
            details.last_successful_build,
            expected_last_successful_build.map(|s| s.to_string()),
        );
        Ok(())
    }

    #[test]
    fn test_last_successful_build_when_last_releases_failed_or_yanked() {
        wrapper(|env| {
            let db = env.db();

            env.fake_release().name("foo").version("0.0.1").create()?;
            env.fake_release().name("foo").version("0.0.2").create()?;
            env.fake_release()
                .name("foo")
                .version("0.0.3")
                .build_result_successful(false)
                .create()?;
            env.fake_release()
                .name("foo")
                .version("0.0.4")
                .yanked(true)
                .create()?;
            env.fake_release()
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
        wrapper(|env| {
            let db = env.db();

            env.fake_release()
                .name("foo")
                .version("0.0.1")
                .build_result_successful(false)
                .create()?;
            env.fake_release()
                .name("foo")
                .version("0.0.2")
                .build_result_successful(false)
                .create()?;
            env.fake_release()
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
        wrapper(|env| {
            let db = env.db();

            env.fake_release().name("foo").version("0.0.1").create()?;
            env.fake_release()
                .name("foo")
                .version("0.0.2")
                .build_result_successful(false)
                .create()?;
            env.fake_release()
                .name("foo")
                .version("0.0.3")
                .yanked(true)
                .create()?;
            env.fake_release().name("foo").version("0.0.4").create()?;

            assert_last_successful_build_equals(&db, "foo", "0.0.1", None)?;
            assert_last_successful_build_equals(&db, "foo", "0.0.2", Some("0.0.4"))?;
            assert_last_successful_build_equals(&db, "foo", "0.0.3", None)?;
            assert_last_successful_build_equals(&db, "foo", "0.0.4", None)?;
            Ok(())
        });
    }

    #[test]
    fn test_releases_should_be_sorted() {
        wrapper(|env| {
            let db = env.db();

            // Add new releases of 'foo' out-of-order since CrateDetails should sort them descending
            env.fake_release().name("foo").version("0.1.0").create()?;
            env.fake_release().name("foo").version("0.1.1").create()?;
            env.fake_release()
                .name("foo")
                .version("0.3.0")
                .build_result_successful(false)
                .create()?;
            env.fake_release().name("foo").version("1.0.0").create()?;
            env.fake_release().name("foo").version("0.12.0").create()?;
            env.fake_release()
                .name("foo")
                .version("0.2.0")
                .yanked(true)
                .create()?;
            env.fake_release()
                .name("foo")
                .version("0.2.0-alpha")
                .create()?;
            env.fake_release()
                .name("foo")
                .version("0.0.1")
                .build_result_successful(false)
                .binary(true)
                .create()?;

            let details = CrateDetails::new(&mut db.conn(), "foo", "0.2.0").unwrap();
            assert_eq!(
                details.releases,
                vec![
                    Release {
                        version: semver::Version::parse("1.0.0")?,
                        build_status: true,
                        yanked: false,
                        is_library: true,
                    },
                    Release {
                        version: semver::Version::parse("0.12.0")?,
                        build_status: true,
                        yanked: false,
                        is_library: true,
                    },
                    Release {
                        version: semver::Version::parse("0.3.0")?,
                        build_status: false,
                        yanked: false,
                        is_library: true,
                    },
                    Release {
                        version: semver::Version::parse("0.2.0")?,
                        build_status: true,
                        yanked: true,
                        is_library: true,
                    },
                    Release {
                        version: semver::Version::parse("0.2.0-alpha")?,
                        build_status: true,
                        yanked: false,
                        is_library: true,
                    },
                    Release {
                        version: semver::Version::parse("0.1.1")?,
                        build_status: true,
                        yanked: false,
                        is_library: true,
                    },
                    Release {
                        version: semver::Version::parse("0.1.0")?,
                        build_status: true,
                        yanked: false,
                        is_library: true,
                    },
                    Release {
                        version: semver::Version::parse("0.0.1")?,
                        build_status: false,
                        yanked: false,
                        is_library: false,
                    },
                ]
            );

            Ok(())
        });
    }

    #[test]
    fn test_latest_version() {
        wrapper(|env| {
            let db = env.db();

            env.fake_release().name("foo").version("0.0.1").create()?;
            env.fake_release().name("foo").version("0.0.3").create()?;
            env.fake_release().name("foo").version("0.0.2").create()?;

            for version in &["0.0.1", "0.0.2", "0.0.3"] {
                let details = CrateDetails::new(&mut db.conn(), "foo", version).unwrap();
                assert_eq!(
                    details.latest_release().version,
                    semver::Version::parse("0.0.3")?
                );
            }

            Ok(())
        })
    }

    #[test]
    fn test_latest_version_ignores_prerelease() {
        wrapper(|env| {
            let db = env.db();

            env.fake_release().name("foo").version("0.0.1").create()?;
            env.fake_release()
                .name("foo")
                .version("0.0.3-pre.1")
                .create()?;
            env.fake_release().name("foo").version("0.0.2").create()?;

            for version in &["0.0.1", "0.0.2", "0.0.3-pre.1"] {
                let details = CrateDetails::new(&mut db.conn(), "foo", version).unwrap();
                assert_eq!(
                    details.latest_release().version,
                    semver::Version::parse("0.0.2")?
                );
            }

            Ok(())
        })
    }

    #[test]
    fn test_latest_version_ignores_yanked() {
        wrapper(|env| {
            let db = env.db();

            env.fake_release().name("foo").version("0.0.1").create()?;
            env.fake_release()
                .name("foo")
                .version("0.0.3")
                .yanked(true)
                .create()?;
            env.fake_release().name("foo").version("0.0.2").create()?;

            for version in &["0.0.1", "0.0.2", "0.0.3"] {
                let details = CrateDetails::new(&mut db.conn(), "foo", version).unwrap();
                assert_eq!(
                    details.latest_release().version,
                    semver::Version::parse("0.0.2")?
                );
            }

            Ok(())
        })
    }

    #[test]
    fn test_latest_version_only_yanked() {
        wrapper(|env| {
            let db = env.db();

            env.fake_release()
                .name("foo")
                .version("0.0.1")
                .yanked(true)
                .create()?;
            env.fake_release()
                .name("foo")
                .version("0.0.3")
                .yanked(true)
                .create()?;
            env.fake_release()
                .name("foo")
                .version("0.0.2")
                .yanked(true)
                .create()?;

            for version in &["0.0.1", "0.0.2", "0.0.3"] {
                let details = CrateDetails::new(&mut db.conn(), "foo", version).unwrap();
                assert_eq!(
                    details.latest_release().version,
                    semver::Version::parse("0.0.3")?
                );
            }

            Ok(())
        })
    }

    #[test]
    fn releases_dropdowns_is_correct() {
        wrapper(|env| {
            env.fake_release()
                .name("binary")
                .version("0.1.0")
                .binary(true)
                .create()?;

            let page = kuchiki::parse_html()
                .one(env.frontend().get("/crate/binary/0.1.0").send()?.text()?);
            let warning = page.select_first("a.pure-menu-link.warn").unwrap();

            assert_eq!(
                warning
                    .as_node()
                    .as_element()
                    .unwrap()
                    .attributes
                    .borrow()
                    .get("title")
                    .unwrap(),
                "binary-0.1.0 is not a library"
            );

            Ok(())
        });
    }

    #[test]
    fn test_updating_owners() {
        wrapper(|env| {
            let db = env.db();

            env.fake_release()
                .name("foo")
                .version("0.0.1")
                .add_owner(CrateOwner {
                    login: "foobar".into(),
                    avatar: "https://example.org/foobar".into(),
                    name: "Foo Bar".into(),
                    email: "foobar@example.org".into(),
                })
                .create()?;

            let details = CrateDetails::new(&mut db.conn(), "foo", "0.0.1").unwrap();
            assert_eq!(
                details.owners,
                vec![("foobar".into(), "https://example.org/foobar".into())]
            );

            // Adding a new owner, and changing details on an existing owner
            env.fake_release()
                .name("foo")
                .version("0.0.2")
                .add_owner(CrateOwner {
                    login: "foobar".into(),
                    avatar: "https://example.org/foobarv2".into(),
                    name: "Foo Bar".into(),
                    email: "foobar@example.org".into(),
                })
                .add_owner(CrateOwner {
                    login: "barfoo".into(),
                    avatar: "https://example.org/barfoo".into(),
                    name: "Bar Foo".into(),
                    email: "foobar@example.org".into(),
                })
                .create()?;

            let details = CrateDetails::new(&mut db.conn(), "foo", "0.0.1").unwrap();
            let mut owners = details.owners;
            owners.sort();
            assert_eq!(
                owners,
                vec![
                    ("barfoo".into(), "https://example.org/barfoo".into()),
                    ("foobar".into(), "https://example.org/foobarv2".into())
                ]
            );

            // Removing an existing owner
            env.fake_release()
                .name("foo")
                .version("0.0.3")
                .add_owner(CrateOwner {
                    login: "barfoo".into(),
                    avatar: "https://example.org/barfoo".into(),
                    name: "Bar Foo".into(),
                    email: "foobar@example.org".into(),
                })
                .create()?;

            let details = CrateDetails::new(&mut db.conn(), "foo", "0.0.1").unwrap();
            assert_eq!(
                details.owners,
                vec![("barfoo".into(), "https://example.org/barfoo".into())]
            );

            // Changing owner details on another of their crates applies the change to both
            env.fake_release()
                .name("bar")
                .version("0.0.1")
                .add_owner(CrateOwner {
                    login: "barfoo".into(),
                    avatar: "https://example.org/barfoov2".into(),
                    name: "Bar Foo".into(),
                    email: "foobar@example.org".into(),
                })
                .create()?;

            let details = CrateDetails::new(&mut db.conn(), "foo", "0.0.1").unwrap();
            assert_eq!(
                details.owners,
                vec![("barfoo".into(), "https://example.org/barfoov2".into())]
            );

            Ok(())
        });
    }
}
