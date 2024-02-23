use crate::{
    impl_axum_webpage,
    web::{
        error::{AxumNope, AxumResult},
        extractors::{DbConnection, Path},
        file::File,
        MetaData,
    },
    AsyncStorage, Config,
};
use anyhow::Context as _;
use axum::{extract::Extension, response::IntoResponse};
use chrono::{DateTime, Utc};
use semver::Version;
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
    use_direct_platform_links: bool,
}

impl_axum_webpage! {
    BuildDetailsPage = "crate/build_details.html",
}

pub(crate) async fn build_details_handler(
    Path((name, version, id)): Path<(String, Version, String)>,
    mut conn: DbConnection,
    Extension(config): Extension<Arc<Config>>,
    Extension(storage): Extension<Arc<AsyncStorage>>,
) -> AxumResult<impl IntoResponse> {
    let id: i32 = id.parse().map_err(|_| AxumNope::BuildNotFound)?;

    let row = sqlx::query!(
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
        id,
        name,
        version.to_string(),
    )
    .fetch_optional(&mut *conn)
    .await?
    .ok_or(AxumNope::BuildNotFound)?;

    let output = if let Some(output) = row.output {
        output
    } else {
        let path = format!("build-logs/{id}/{}.txt", row.default_target);
        let file = File::from_path(&storage, &path, &config).await?;
        String::from_utf8(file.0.content).context("non utf8")?
    };

    Ok(BuildDetailsPage {
        metadata: MetaData::from_crate(&mut conn, &name, &version, None).await?,
        build_details: BuildDetails {
            id,
            rustc_version: row.rustc_version,
            docsrs_version: row.docsrs_version,
            build_status: row.build_status,
            build_time: row.build_time,
            output,
        },
        use_direct_platform_links: true,
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

            let page = kuchikiki::parse_html()
                .one(env.frontend().get(url).send()?.error_for_status()?.text()?);

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
