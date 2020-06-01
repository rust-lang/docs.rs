use super::{ApiErrorV1, Pool};
use iron::Plugin;
use iron::{headers::ContentType, status, IronResult, Request, Response};
use params::{Params, Value};
use serde::{Deserialize, Serialize};

/// The json data of a crate release
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct BadgeV1 {
    /// The name of the crate
    name: String,
    /// The version of the release
    version: String,
    /// The url of the crate
    docsrs_url: String,
    /// The crate's status
    build_status: BuildStatus,
}

/// The status of a crate release
#[derive(Debug, Copy, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename = "snake_case")]
enum BuildStatus {
    /// The release successfully built
    Built,
    /// The release failed to built
    Failed,
    /// The release is queued to build
    Queued,
    /// The release was yanked
    Yanked,
    /// The release is not a library
    NotLibrary,
}

/// The badge api, expects `?crate=<string>` and an optional `version=<string>`, if the version is not
/// provided then the latest release will be used
pub fn badge_handler_v1(req: &mut Request) -> IronResult<Response> {
    let params = ctry!(req.get::<Params>());
    let conn = extension!(req, Pool).get()?;

    let krate = match params.find(&["crate"]) {
        Some(Value::String(krate)) => krate,
        Some(..) => return ApiErrorV1::new("Crate must be a string").into_response(),
        None => return ApiErrorV1::new("A crate was not supplied").into_response(),
    };
    let version = match params.find(&["version"]) {
        Some(Value::String(version)) => Some(version),
        Some(..) => return ApiErrorV1::new("Version must be a string").into_response(),
        None => None,
    };

    // If a version was provided, then attempt to find the release for that crate & version
    let (rows, version) = if let Some(version) = version {
        let query = "
            SELECT is_library, rustdoc_status, build_status, yanked
            FROM releases
            WHERE crate_id IN (
                SELECT id from crates
                WHERE name = $1 AND version = $2
            )";
        let rows = ctry!(conn.query(query, &[&krate, &version]));

        (rows, version.to_owned())

    // If no version was provided, find the latest release and use that
    } else {
        let query = "
            SELECT is_library, rustdoc_status, build_status, yanked, version
            FROM releases
            WHERE id IN (
                SELECT latest_version_id from crates
                WHERE name = $1
            )";
        let rows = ctry!(conn.query(query, &[&krate]));

        let version = api_error!(
            rows.iter().next().map(|r| r.get::<_, String>("version")),
            "The requested crate does not exist",
        );

        (rows, version)
    };

    // If the crate & version is found in the database, it's built in some form
    let build_status = if let Some(release) = rows.iter().next() {
        // If the release isn't a library
        if !release.get::<_, bool>("is_library") {
            BuildStatus::NotLibrary

        // If the release was yanked
        } else if release.get("yanked") {
            BuildStatus::Yanked

        // If the build succeeded
        } else if release.get("rustdoc_status") || release.get("build_status") {
            BuildStatus::Built

        // If none of the above, then the build failed in some way
        } else {
            BuildStatus::Failed
        }

    // If we can't find the crate & version in the db, it might be in the queue
    } else {
        let query = "SELECT COUNT(*) AS count FROM queue WHERE name = $1 AND version = $2";
        let count: i64 = ctry!(conn.query(query, &[&krate, &version]))
            .iter()
            .next()
            .map(|r| r.get("count"))
            .unwrap_or_default();

        // If there's an entry for the crate in the build queue, it's queued
        if count != 0 {
            BuildStatus::Queued
        } else {
            return ApiErrorV1::new("The requested crate does not exist").into_response();
        }
    };

    // Form the url of the crate
    let docsrs_url = format!("https://docs.rs/crate/{}/{}", krate, version);

    let badge = BadgeV1 {
        name: krate.to_owned(),
        version,
        docsrs_url,
        build_status,
    };

    let mut resp = Response::with((status::Ok, serde_json::to_string(&badge).unwrap()));
    resp.headers
        .set(ContentType("application/json".parse().unwrap()));

    Ok(resp)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test::wrapper;

    #[test]
    fn get_latest() {
        wrapper(|env| {
            let web = env.frontend();
            let db = env.db();

            db.fake_release()
                .name("saltwater")
                .version("1.4.0")
                .create()?;

            let mut resp = web.get("/api/v1/badges?crate=saltwater").send()?;

            assert!(resp.status().is_success());
            assert_eq!(
                resp.json::<BadgeV1>()?,
                BadgeV1 {
                    name: "saltwater".to_owned(),
                    version: "1.4.0".to_owned(),
                    docsrs_url: "https://docs.rs/crate/saltwater/1.4.0".to_owned(),
                    build_status: BuildStatus::Built,
                },
            );

            Ok(())
        });
    }

    #[test]
    fn crate_not_found() {
        let expected = ApiErrorV1::new("The requested crate does not exist");

        wrapper(|env| {
            let web = env.frontend();
            let db = env.db();

            let mut resp = web
                .get("/api/v1/badges?crate=saltwater&version=0.0.0")
                .send()?;
            assert!(resp.status().is_client_error());
            assert_eq!(serde_json::from_str::<ApiErrorV1>(&resp.text()?)?, expected);

            let mut resp = web.get("/api/v1/badges?crate=saltwater").send()?;
            assert!(resp.status().is_client_error());
            assert_eq!(serde_json::from_str::<ApiErrorV1>(&resp.text()?)?, expected);

            db.fake_release()
                .name("saltwater")
                .version("1.4.0")
                .create()?;

            let mut resp = web
                .get("/api/v1/badges?crate=saltwater&version=0.0.0")
                .send()?;
            assert!(resp.status().is_client_error());
            assert_eq!(serde_json::from_str::<ApiErrorV1>(&resp.text()?)?, expected);

            Ok(())
        });
    }

    #[test]
    fn crate_not_provided() {
        let expected = ApiErrorV1::new("A crate was not supplied");

        wrapper(|env| {
            let web = env.frontend();

            let mut resp = web.get("/api/v1/badges").send()?;
            assert!(resp.status().is_client_error());
            assert_eq!(resp.json::<ApiErrorV1>()?, expected);

            Ok(())
        });
    }

    #[test]
    fn crate_built() {
        wrapper(|env| {
            let db = env.db();
            let web = env.frontend();

            db.fake_release()
                .name("saltwater")
                .version("1.4.0")
                .create()?;

            let mut resp = web
                .get("/api/v1/badges?crate=saltwater&version=1.4.0")
                .send()?;

            assert!(resp.status().is_success());
            assert_eq!(
                resp.json::<BadgeV1>()?,
                BadgeV1 {
                    name: "saltwater".to_owned(),
                    version: "1.4.0".to_owned(),
                    docsrs_url: "https://docs.rs/crate/saltwater/1.4.0".to_owned(),
                    build_status: BuildStatus::Built,
                },
            );

            Ok(())
        });
    }

    #[test]
    fn crate_yanked() {
        wrapper(|env| {
            let db = env.db();
            let web = env.frontend();

            db.fake_release()
                .name("saltwater")
                .version("1.6.0")
                .yanked(true)
                .create()?;

            let mut resp = web
                .get("/api/v1/badges?crate=saltwater&version=1.6.0")
                .send()?;

            assert!(resp.status().is_success());
            assert_eq!(
                resp.json::<BadgeV1>()?,
                BadgeV1 {
                    name: "saltwater".to_owned(),
                    version: "1.6.0".to_owned(),
                    docsrs_url: "https://docs.rs/crate/saltwater/1.6.0".to_owned(),
                    build_status: BuildStatus::Yanked,
                },
            );

            Ok(())
        });
    }

    #[test]
    fn crate_failed() {
        wrapper(|env| {
            let db = env.db();
            let web = env.frontend();

            db.fake_release()
                .name("saltwater")
                .version("1.7.0")
                .build_result_successful(false)
                .create()?;

            let mut resp = web
                .get("/api/v1/badges?crate=saltwater&version=1.7.0")
                .send()?;

            assert!(resp.status().is_success());
            assert_eq!(
                resp.json::<BadgeV1>()?,
                BadgeV1 {
                    name: "saltwater".to_owned(),
                    version: "1.7.0".to_owned(),
                    docsrs_url: "https://docs.rs/crate/saltwater/1.7.0".to_owned(),
                    build_status: BuildStatus::Failed,
                },
            );

            Ok(())
        });
    }

    #[test]
    fn crate_in_queue() {
        wrapper(|env| {
            let db = env.db();
            let web = env.frontend();

            crate::utils::add_crate_to_queue(&*db.conn(), "saltwater", "1.8.0", 1)?;

            let mut resp = web
                .get("/api/v1/badges?crate=saltwater&version=1.8.0")
                .send()?;

            assert!(resp.status().is_success());
            assert_eq!(
                resp.json::<BadgeV1>()?,
                BadgeV1 {
                    name: "saltwater".to_owned(),
                    version: "1.8.0".to_owned(),
                    docsrs_url: "https://docs.rs/crate/saltwater/1.8.0".to_owned(),
                    build_status: BuildStatus::Queued,
                },
            );

            Ok(())
        });
    }
}
