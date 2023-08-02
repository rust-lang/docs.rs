use crate::{
    db::Pool,
    impl_axum_webpage,
    utils::spawn_blocking,
    web::{
        error::{AxumNope, AxumResult},
        file::File,
        MetaData,
    },
    Config, Storage,
};
use axum::{
    extract::{Extension, Path},
    response::IntoResponse,
};
use chrono::{DateTime, Utc};
use serde::Serialize;
use std::sync::Arc;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct BuildDetails {
    id: i32,
    rustc_version: String,
    docsrs_version: String,
    build_status: bool,
    build_time: DateTime<Utc>,
    output: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct BuildDetailsPage {
    metadata: MetaData,
    build_details: BuildDetails,
}

impl_axum_webpage! {
    BuildDetailsPage = "crate/build_details.html",
}

pub(crate) async fn build_details_handler(
    Path((name, version, id)): Path<(String, String, String)>,
    Extension(pool): Extension<Pool>,
    Extension(config): Extension<Arc<Config>>,
    Extension(storage): Extension<Arc<Storage>>,
) -> AxumResult<impl IntoResponse> {
    let id: i32 = id.parse().map_err(|_| AxumNope::BuildNotFound)?;

    let (row, output, metadata) = spawn_blocking(move || {
        let mut conn = pool.get()?;
        let row = conn
            .query_opt(
                "SELECT
                     builds.rustc_version,
                     builds.docsrs_version,
                     builds.build_status,
                     builds.build_time,
                     builds.output,
                     releases.default_target
                 FROM builds
                 INNER JOIN releases ON releases.id = builds.rid
                 INNER JOIN crates ON releases.crate_id = crates.id
                 WHERE builds.id = $1 AND crates.name = $2 AND releases.version = $3",
                &[&id, &name, &version],
            )?
            .ok_or(AxumNope::BuildNotFound)?;

        let output = if let Some(output) = row.get("output") {
            output
        } else {
            let target: String = row.get("default_target");
            let path = format!("build-logs/{id}/{target}.txt");
            let file = File::from_path(&storage, &path, &config)?;
            String::from_utf8(file.0.content)?
        };

        Ok((
            row,
            output,
            MetaData::from_crate(&mut conn, &name, &version, &version)?,
        ))
    })
    .await?;

    Ok(BuildDetailsPage {
        metadata,
        build_details: BuildDetails {
            id,
            rustc_version: row.get("rustc_version"),
            docsrs_version: row.get("docsrs_version"),
            build_status: row.get("build_status"),
            build_time: row.get("build_time"),
            output,
        },
    }
    .into_response())
}

#[cfg(test)]
mod tests {
    use crate::test::{wrapper, FakeBuild};
    use kuchikiki::traits::TendrilSink;
    use test_case::test_case;

    #[test]
    fn db_build_logs() {
        wrapper(|env| {
            env.fake_release()
                .name("foo")
                .version("0.1.0")
                .builds(vec![FakeBuild::default()
                    .no_s3_build_log()
                    .db_build_log("A build log")])
                .create()?;

            let page = kuchikiki::parse_html().one(
                env.frontend()
                    .get("/crate/foo/0.1.0/builds")
                    .send()?
                    .text()?,
            );

            let node = page.select("ul > li a.release").unwrap().next().unwrap();
            let attrs = node.attributes.borrow();
            let url = attrs.get("href").unwrap();

            let page = kuchikiki::parse_html().one(env.frontend().get(url).send()?.text()?);

            let log = page.select("pre").unwrap().next().unwrap().text_contents();

            assert!(log.contains("A build log"));

            Ok(())
        });
    }

    #[test]
    fn s3_build_logs() {
        wrapper(|env| {
            env.fake_release()
                .name("foo")
                .version("0.1.0")
                .builds(vec![FakeBuild::default().s3_build_log("A build log")])
                .create()?;

            let page = kuchikiki::parse_html().one(
                env.frontend()
                    .get("/crate/foo/0.1.0/builds")
                    .send()?
                    .text()?,
            );

            let node = page.select("ul > li a.release").unwrap().next().unwrap();
            let attrs = node.attributes.borrow();
            let url = attrs.get("href").unwrap();

            let page = kuchikiki::parse_html().one(env.frontend().get(url).send()?.text()?);

            let log = page.select("pre").unwrap().next().unwrap().text_contents();

            assert!(log.contains("A build log"));

            Ok(())
        });
    }

    #[test]
    fn both_build_logs() {
        wrapper(|env| {
            env.fake_release()
                .name("foo")
                .version("0.1.0")
                .builds(vec![FakeBuild::default()
                    .s3_build_log("A build log")
                    .db_build_log("Another build log")])
                .create()?;

            let page = kuchikiki::parse_html().one(
                env.frontend()
                    .get("/crate/foo/0.1.0/builds")
                    .send()?
                    .text()?,
            );

            let node = page.select("ul > li a.release").unwrap().next().unwrap();
            let attrs = node.attributes.borrow();
            let url = attrs.get("href").unwrap();

            let page = kuchikiki::parse_html().one(env.frontend().get(url).send()?.text()?);

            let log = page.select("pre").unwrap().next().unwrap().text_contents();

            // Relatively arbitrarily the DB is prioritised
            assert!(log.contains("Another build log"));

            Ok(())
        });
    }

    #[test_case("42")]
    #[test_case("nan")]
    fn non_existing_build(build_id: &str) {
        wrapper(|env| {
            env.fake_release().name("foo").version("0.1.0").create()?;

            let res = env
                .frontend()
                .get(&format!("/crate/foo/0.1.0/builds/{build_id}"))
                .send()?;
            assert_eq!(res.status(), 404);
            assert!(res.text()?.contains("no such build"));

            Ok(())
        });
    }
}
