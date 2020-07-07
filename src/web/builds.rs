use super::duration_to_str;
use super::page::Page;
use super::MetaData;
use crate::db::Pool;
use crate::docbuilder::Limits;
use chrono::{DateTime, NaiveDateTime, Utc};
use iron::prelude::*;
use router::Router;
use serde::ser::{Serialize, SerializeStruct, Serializer};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Build {
    id: i32,
    rustc_version: String,
    cratesfyi_version: String,
    build_status: bool,
    build_time: DateTime<Utc>,
    output: Option<String>,
}

impl Serialize for Build {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut state = serializer.serialize_struct("Build", 7)?;
        state.serialize_field("id", &self.id)?;
        state.serialize_field("rustc_version", &self.rustc_version)?;
        state.serialize_field("cratesfyi_version", &self.cratesfyi_version)?;
        state.serialize_field("build_status", &self.build_status)?;
        state.serialize_field("build_time", &self.build_time.format("%+").to_string())?; // RFC 3339
        state.serialize_field("build_time_relative", &duration_to_str(self.build_time))?;
        state.serialize_field("output", &self.output)?;

        state.end()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct BuildsPage {
    metadata: Option<MetaData>,
    builds: Vec<Build>,
    build_details: Option<Build>,
    limits: Limits,
}

impl Serialize for BuildsPage {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut state = serializer.serialize_struct("Buildspage", 4)?;
        state.serialize_field("metadata", &self.metadata)?;
        state.serialize_field("builds", &self.builds)?;
        state.serialize_field("build_details", &self.build_details)?;
        state.serialize_field("limits", &self.limits.for_website())?;

        state.end()
    }
}

pub fn build_list_handler(req: &mut Request) -> IronResult<Response> {
    let router = extension!(req, Router);
    let name = cexpect!(router.find("name"));
    let version = cexpect!(router.find("version"));
    let req_build_id: i32 = router.find("id").unwrap_or("0").parse().unwrap_or(0);

    let conn = extension!(req, Pool).get()?;
    let limits = ctry!(Limits::for_crate(&conn, name));

    let query = ctry!(conn.query(
        "SELECT crates.name,
                releases.version,
                releases.description,
                releases.rustdoc_status,
                releases.target_name,
                builds.id,
                builds.rustc_version,
                builds.cratesfyi_version,
                builds.build_status,
                builds.build_time,
                builds.output
         FROM builds
         INNER JOIN releases ON releases.id = builds.rid
         INNER JOIN crates ON releases.crate_id = crates.id
         WHERE crates.name = $1 AND releases.version = $2
         ORDER BY id DESC",
        &[&name, &version]
    ));

    let mut build_details = None;
    // FIXME: getting builds.output may cause performance issues when release have tons of builds
    let mut build_list = query
        .into_iter()
        .map(|row| {
            let id: i32 = row.get(5);

            let build = Build {
                id,
                rustc_version: row.get(6),
                cratesfyi_version: row.get(7),
                build_status: row.get(8),
                build_time: DateTime::from_utc(row.get::<_, NaiveDateTime>(9), Utc),
                output: row.get(10),
            };

            if id == req_build_id {
                build_details = Some(build.clone());
            }

            build
        })
        .collect::<Vec<Build>>();

    if req.url.path().join("/").ends_with(".json") {
        use iron::headers::{
            AccessControlAllowOrigin, CacheControl, CacheDirective, ContentType, Expires, HttpDate,
        };
        use iron::status;

        // Remove build output from build list for json output
        for build in build_list.as_mut_slice() {
            build.output = None;
        }

        let mut resp = Response::with((status::Ok, serde_json::to_string(&build_list).unwrap()));
        resp.headers
            .set(ContentType("application/json".parse().unwrap()));
        resp.headers.set(Expires(HttpDate(time::now())));
        resp.headers.set(CacheControl(vec![
            CacheDirective::NoCache,
            CacheDirective::NoStore,
            CacheDirective::MustRevalidate,
        ]));
        resp.headers.set(AccessControlAllowOrigin::Any);
        Ok(resp)
    } else {
        let builds_page = BuildsPage {
            metadata: MetaData::from_crate(&conn, &name, &version),
            builds: build_list,
            build_details,
            limits,
        };
        Page::new(builds_page)
            .set_true("show_package_navigation")
            .set_true("package_navigation_builds_tab")
            .to_resp("builds")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use serde_json::json;

    #[test]
    fn serialize_build() {
        let time = Utc::now();
        let mut build = Build {
            id: 22,
            rustc_version: "rustc 1.43.0 (4fb7144ed 2020-04-20)".to_string(),
            cratesfyi_version: "docsrs 0.6.0 (3dd32ec 2020-05-01)".to_string(),
            build_status: true,
            build_time: time,
            output: None,
        };

        let correct_json = json!({
            "id": 22,
            "rustc_version": "rustc 1.43.0 (4fb7144ed 2020-04-20)",
            "cratesfyi_version": "docsrs 0.6.0 (3dd32ec 2020-05-01)",
            "build_time": time.format("%+").to_string(),
            "build_time_relative": duration_to_str(time),
            "output": null,
            "build_status": true
        });

        assert_eq!(correct_json, serde_json::to_value(&build).unwrap());

        build.output = Some("some random stuff".to_string());
        let correct_json = json!({
            "id": 22,
            "rustc_version": "rustc 1.43.0 (4fb7144ed 2020-04-20)",
            "cratesfyi_version": "docsrs 0.6.0 (3dd32ec 2020-05-01)",
            "build_time": time.format("%+").to_string(),
            "build_time_relative": duration_to_str(time),
            "output": "some random stuff",
            "build_status": true
        });

        assert_eq!(correct_json, serde_json::to_value(&build).unwrap());
    }

    #[test]
    fn serialize_build_page() {
        let time = Utc::now();
        let build = Build {
            id: 22,
            rustc_version: "rustc 1.43.0 (4fb7144ed 2020-04-20)".to_string(),
            cratesfyi_version: "docsrs 0.6.0 (3dd32ec 2020-05-01)".to_string(),
            build_status: true,
            build_time: time,
            output: None,
        };
        let limits = Limits::default();
        let mut builds = BuildsPage {
            metadata: Some(MetaData {
                name: "serde".to_string(),
                version: "1.0.0".to_string(),
                description: Some("serde does stuff".to_string()),
                target_name: None,
                rustdoc_status: true,
                default_target: "x86_64-unknown-linux-gnu".to_string(),
            }),
            builds: vec![build.clone()],
            build_details: Some(build.clone()),
            limits: limits.clone(),
        };

        let correct_json = json!({
            "metadata": {
                "name": "serde",
                "version": "1.0.0",
                "description": "serde does stuff",
                "target_name": null,
                "rustdoc_status": true,
                "default_target": "x86_64-unknown-linux-gnu"
            },
            "builds": [{
                "id": 22,
                "rustc_version": "rustc 1.43.0 (4fb7144ed 2020-04-20)",
                "cratesfyi_version": "docsrs 0.6.0 (3dd32ec 2020-05-01)",
                "build_time": time.format("%+").to_string(),
                "build_time_relative": duration_to_str(time),
                "output": null,
                "build_status": true
            }],
            "build_details": {
                "id": 22,
                "rustc_version": "rustc 1.43.0 (4fb7144ed 2020-04-20)",
                "cratesfyi_version": "docsrs 0.6.0 (3dd32ec 2020-05-01)",
                "build_time": time.format("%+").to_string(),
                "build_time_relative": duration_to_str(time),
                "output": null,
                "build_status": true
            },
            "limits": limits.for_website(),
        });

        assert_eq!(correct_json, serde_json::to_value(&builds).unwrap());

        builds.metadata = None;
        let correct_json = json!({
            "metadata": null,
            "builds": [{
                "id": 22,
                "rustc_version": "rustc 1.43.0 (4fb7144ed 2020-04-20)",
                "cratesfyi_version": "docsrs 0.6.0 (3dd32ec 2020-05-01)",
                "build_time": time.format("%+").to_string(),
                "build_time_relative": duration_to_str(time),
                "output": null,
                "build_status": true
            }],
            "build_details": {
                "id": 22,
                "rustc_version": "rustc 1.43.0 (4fb7144ed 2020-04-20)",
                "cratesfyi_version": "docsrs 0.6.0 (3dd32ec 2020-05-01)",
                "build_time": time.format("%+").to_string(),
                "build_time_relative": duration_to_str(time),
                "output": null,
                "build_status": true
            },
            "limits": limits.for_website(),
        });

        assert_eq!(correct_json, serde_json::to_value(&builds).unwrap());

        builds.builds = Vec::new();
        let correct_json = json!({
            "metadata": null,
            "builds": [],
            "build_details": {
                "id": 22,
                "rustc_version": "rustc 1.43.0 (4fb7144ed 2020-04-20)",
                "cratesfyi_version": "docsrs 0.6.0 (3dd32ec 2020-05-01)",
                "build_time": time.format("%+").to_string(),
                "build_time_relative": duration_to_str(time),
                "output": null,
                "build_status": true
            },
            "limits": limits.for_website()
        });

        assert_eq!(correct_json, serde_json::to_value(&builds).unwrap());

        builds.build_details = None;
        let correct_json = json!({
            "metadata": null,
            "builds": [],
            "build_details": null,
            "limits": limits.for_website(),
        });

        assert_eq!(correct_json, serde_json::to_value(&builds).unwrap());
    }
}
