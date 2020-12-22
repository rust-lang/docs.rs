use crate::{
    db::Pool,
    docbuilder::Limits,
    impl_webpage,
    web::{page::WebPage, MetaData},
};
use chrono::{DateTime, Utc};
use iron::{
    headers::{
        AccessControlAllowOrigin, CacheControl, CacheDirective, ContentType, Expires, HttpDate,
    },
    status, IronResult, Request, Response,
};
use router::Router;
use serde::Serialize;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct Build {
    id: i32,
    rustc_version: String,
    docsrs_version: String,
    build_status: bool,
    build_time: DateTime<Utc>,
    output: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct BuildsPage {
    metadata: MetaData,
    builds: Vec<Build>,
    build_details: Option<Build>,
    limits: Limits,
}

impl_webpage! {
    BuildsPage = "crate/builds.html",
}

pub fn build_list_handler(req: &mut Request) -> IronResult<Response> {
    let router = extension!(req, Router);
    let name = cexpect!(req, router.find("name"));
    let version = cexpect!(req, router.find("version"));
    let req_build_id: i32 = router.find("id").unwrap_or("0").parse().unwrap_or(0);

    let mut conn = extension!(req, Pool).get()?;
    let limits = ctry!(req, Limits::for_crate(&mut conn, name));

    let query = ctry!(
        req,
        conn.query(
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
        )
    );

    let mut build_details = None;
    // FIXME: getting builds.output may cause performance issues when release have tons of builds
    let mut builds = query
        .into_iter()
        .map(|row| {
            let id: i32 = row.get("id");

            let build = Build {
                id,
                rustc_version: row.get("rustc_version"),
                docsrs_version: row.get("cratesfyi_version"),
                build_status: row.get("build_status"),
                build_time: row.get("build_time"),
                output: row.get("output"),
            };

            if id == req_build_id {
                build_details = Some(build.clone());
            }

            build
        })
        .collect::<Vec<Build>>();

    if req.url.path().join("/").ends_with(".json") {
        // Remove build output from build list for json output
        for build in builds.iter_mut() {
            build.output = None;
        }

        let mut resp = Response::with((status::Ok, serde_json::to_string(&builds).unwrap()));
        resp.headers.set(ContentType::json());
        resp.headers.set(Expires(HttpDate(time::now())));
        resp.headers.set(CacheControl(vec![
            CacheDirective::NoCache,
            CacheDirective::NoStore,
            CacheDirective::MustRevalidate,
        ]));
        resp.headers.set(AccessControlAllowOrigin::Any);

        Ok(resp)
    } else {
        BuildsPage {
            metadata: cexpect!(req, MetaData::from_crate(&mut conn, &name, &version)),
            builds,
            build_details,
            limits,
        }
        .into_response(req)
    }
}

#[cfg(test)]
mod tests {
    use crate::test::{wrapper, FakeBuild};
    use chrono::{DateTime, Duration, Utc};
    use kuchiki::traits::TendrilSink;

    #[test]
    fn build_list() {
        wrapper(|env| {
            env.fake_release()
                .name("foo")
                .version("0.1.0")
                .builds(vec![
                    FakeBuild::default()
                        .rustc_version("rustc 1.0.0")
                        .docsrs_version("docs.rs 1.0.0"),
                    FakeBuild::default()
                        .successful(false)
                        .rustc_version("rustc 2.0.0")
                        .docsrs_version("docs.rs 2.0.0"),
                    FakeBuild::default()
                        .rustc_version("rustc 3.0.0")
                        .docsrs_version("docs.rs 3.0.0"),
                ])
                .create()?;

            let page = kuchiki::parse_html().one(
                env.frontend()
                    .get("/crate/foo/0.1.0/builds")
                    .send()?
                    .text()?,
            );

            let rows: Vec<_> = page
                .select("ul > li a.release")
                .unwrap()
                .map(|row| row.text_contents())
                .collect();

            assert!(rows[0].contains("rustc 3.0.0"));
            assert!(rows[0].contains("docs.rs 3.0.0"));
            assert!(rows[1].contains("rustc 2.0.0"));
            assert!(rows[1].contains("docs.rs 2.0.0"));
            assert!(rows[2].contains("rustc 1.0.0"));
            assert!(rows[2].contains("docs.rs 1.0.0"));

            Ok(())
        });
    }

    #[test]
    fn build_list_json() {
        wrapper(|env| {
            env.fake_release()
                .name("foo")
                .version("0.1.0")
                .builds(vec![
                    FakeBuild::default()
                        .rustc_version("rustc 1.0.0")
                        .docsrs_version("docs.rs 1.0.0"),
                    FakeBuild::default()
                        .successful(false)
                        .rustc_version("rustc 2.0.0")
                        .docsrs_version("docs.rs 2.0.0"),
                    FakeBuild::default()
                        .rustc_version("rustc 3.0.0")
                        .docsrs_version("docs.rs 3.0.0"),
                ])
                .create()?;

            let value: serde_json::Value = serde_json::from_str(
                &env.frontend()
                    .get("/crate/foo/0.1.0/builds.json")
                    .send()?
                    .text()?,
            )?;

            assert_eq!(value.pointer("/0/build_status"), Some(&true.into()));
            assert_eq!(
                value.pointer("/0/docsrs_version"),
                Some(&"docs.rs 3.0.0".into())
            );
            assert_eq!(
                value.pointer("/0/rustc_version"),
                Some(&"rustc 3.0.0".into())
            );
            assert!(value.pointer("/0/id").unwrap().is_i64());
            assert!(serde_json::from_value::<DateTime<Utc>>(
                value.pointer("/0/build_time").unwrap().clone()
            )
            .is_ok());

            assert_eq!(value.pointer("/1/build_status"), Some(&false.into()));
            assert_eq!(
                value.pointer("/1/docsrs_version"),
                Some(&"docs.rs 2.0.0".into())
            );
            assert_eq!(
                value.pointer("/1/rustc_version"),
                Some(&"rustc 2.0.0".into())
            );
            assert!(value.pointer("/1/id").unwrap().is_i64());
            assert!(serde_json::from_value::<DateTime<Utc>>(
                value.pointer("/1/build_time").unwrap().clone()
            )
            .is_ok());

            assert_eq!(value.pointer("/2/build_status"), Some(&true.into()));
            assert_eq!(
                value.pointer("/2/docsrs_version"),
                Some(&"docs.rs 1.0.0".into())
            );
            assert_eq!(
                value.pointer("/2/rustc_version"),
                Some(&"rustc 1.0.0".into())
            );
            assert!(value.pointer("/2/id").unwrap().is_i64());
            assert!(serde_json::from_value::<DateTime<Utc>>(
                value.pointer("/2/build_time").unwrap().clone()
            )
            .is_ok());

            assert!(
                value.pointer("/1/build_time").unwrap().as_str().unwrap()
                    < value.pointer("/0/build_time").unwrap().as_str().unwrap()
            );
            assert!(
                value.pointer("/2/build_time").unwrap().as_str().unwrap()
                    < value.pointer("/1/build_time").unwrap().as_str().unwrap()
            );

            Ok(())
        });
    }

    #[test]
    fn limits() {
        wrapper(|env| {
            env.fake_release().name("foo").version("0.1.0").create()?;

            env.db().conn().query(
                "INSERT INTO sandbox_overrides
                    (crate_name, max_memory_bytes, timeout_seconds, max_targets)
                 VALUES ($1, $2, $3, $4)",
                &[
                    &"foo",
                    &3072i64,
                    &(Duration::hours(2).num_seconds() as i32),
                    &1,
                ],
            )?;

            let page = kuchiki::parse_html().one(
                env.frontend()
                    .get("/crate/foo/0.1.0/builds")
                    .send()?
                    .text()?,
            );

            let header = page.select(".about h4").unwrap().next().unwrap();
            assert_eq!(header.text_contents(), "foo's sandbox limits");

            let values: Vec<_> = page
                .select(".about table tr td:last-child")
                .unwrap()
                .map(|row| row.text_contents())
                .collect();
            let values: Vec<_> = values.iter().map(|v| &**v).collect();

            dbg!(&values);
            assert!(values.contains(&"3 KB"));
            assert!(values.contains(&"2 hours"));
            assert!(values.contains(&"100 KB"));
            assert!(values.contains(&"blocked"));
            assert!(values.contains(&"1"));

            Ok(())
        });
    }

    #[test]
    fn build_logs() {
        wrapper(|env| {
            env.fake_release()
                .name("foo")
                .version("0.1.0")
                .builds(vec![FakeBuild::default().build_log("A build log")])
                .create()?;

            let page = kuchiki::parse_html().one(
                env.frontend()
                    .get("/crate/foo/0.1.0/builds")
                    .send()?
                    .text()?,
            );

            let node = page.select("ul > li a.release").unwrap().next().unwrap();
            let attrs = node.attributes.borrow();
            let url = attrs.get("href").unwrap();

            let page = kuchiki::parse_html().one(env.frontend().get(url).send()?.text()?);

            let log = page.select("pre").unwrap().next().unwrap().text_contents();

            assert!(log.contains("A build log"));

            Ok(())
        });
    }
}
