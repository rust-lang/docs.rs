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
use futures_util::TryStreamExt;
use semver::Version;
use serde::{Deserialize, Serialize};
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
    all_log_filenames: Vec<String>,
    current_filename: Option<String>,
}

impl_axum_webpage! {
    BuildDetailsPage = "crate/build_details.html",
}

#[derive(Clone, Deserialize, Debug)]
pub(crate) struct BuildDetailsParams {
    pub(crate) name: String,
    pub(crate) version: Version,
    pub(crate) id: String,
    pub(crate) filename: Option<String>,
}

pub(crate) async fn build_details_handler(
    Path(params): Path<BuildDetailsParams>,
    mut conn: DbConnection,
    Extension(config): Extension<Arc<Config>>,
    Extension(storage): Extension<Arc<AsyncStorage>>,
) -> AxumResult<impl IntoResponse> {
    let id: i32 = params.id.parse().map_err(|_| AxumNope::BuildNotFound)?;

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
        params.name,
        params.version.to_string(),
    )
    .fetch_optional(&mut *conn)
    .await?
    .ok_or(AxumNope::BuildNotFound)?;

    let (output, all_log_filenames, current_filename) = if let Some(output) = row.output {
        (output, Vec::new(), None)
    } else {
        let prefix = format!("build-logs/{}/", id);

        let current_filename = params
            .filename
            .unwrap_or_else(|| format!("{}.txt", row.default_target));

        let path = format!("{prefix}{current_filename}");
        let file = File::from_path(&storage, &path, &config).await?;
        (
            String::from_utf8(file.0.content).context("non utf8")?,
            storage
                .list_prefix(&prefix)
                .await
                .map_ok(|path| {
                    path.strip_prefix(&prefix)
                        .expect("since we query for the prefix, it has to be always there")
                        .to_owned()
                })
                .try_collect()
                .await?,
            Some(current_filename),
        )
    };

    Ok(BuildDetailsPage {
        metadata: MetaData::from_crate(&mut conn, &params.name, &params.version, None).await?,
        build_details: BuildDetails {
            id,
            rustc_version: row.rustc_version,
            docsrs_version: row.docsrs_version,
            build_status: row.build_status,
            build_time: row.build_time,
            output,
        },
        use_direct_platform_links: true,
        all_log_filenames,
        current_filename,
    }
    .into_response())
}

#[cfg(test)]
mod tests {
    use crate::test::{wrapper, FakeBuild};
    use kuchikiki::traits::TendrilSink;
    use test_case::test_case;

    fn get_all_log_links(page: &kuchikiki::NodeRef) -> Vec<(String, String)> {
        page.select("ul > li a.release")
            .unwrap()
            .map(|el| {
                let attributes = el.attributes.borrow();
                (
                    el.text_contents().trim().to_owned(),
                    attributes.get("href").unwrap().to_string(),
                )
            })
            .collect()
    }

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
            assert!(get_all_log_links(&page).is_empty());

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
            let build_url = attrs.get("href").unwrap();

            let page = kuchikiki::parse_html().one(env.frontend().get(build_url).send()?.text()?);

            let log = page.select("pre").unwrap().next().unwrap().text_contents();

            assert!(log.contains("A build log"));

            let all_log_links = get_all_log_links(&page);
            assert_eq!(
                all_log_links,
                vec![(
                    "x86_64-unknown-linux-gnu.txt".into(),
                    format!("{build_url}/x86_64-unknown-linux-gnu.txt")
                )]
            );

            // now get the log with the specific filename in the URL
            let log = kuchikiki::parse_html()
                .one(env.frontend().get(&all_log_links[0].1).send()?.text()?)
                .select("pre")
                .unwrap()
                .next()
                .unwrap()
                .text_contents();

            assert!(log.contains("A build log"));

            Ok(())
        });
    }

    #[test]
    fn s3_build_logs_multiple_targets() {
        wrapper(|env| {
            env.fake_release()
                .name("foo")
                .version("0.1.0")
                .builds(vec![FakeBuild::default()
                    .s3_build_log("A build log")
                    .build_log_for_other_target(
                        "other_target",
                        "other target build log",
                    )])
                .create()?;

            let page = kuchikiki::parse_html().one(
                env.frontend()
                    .get("/crate/foo/0.1.0/builds")
                    .send()?
                    .text()?,
            );

            let node = page.select("ul > li a.release").unwrap().next().unwrap();
            let attrs = node.attributes.borrow();
            let build_url = attrs.get("href").unwrap();

            let page = kuchikiki::parse_html().one(env.frontend().get(build_url).send()?.text()?);

            let log = page.select("pre").unwrap().next().unwrap().text_contents();

            assert!(log.contains("A build log"));

            let all_log_links = get_all_log_links(&page);
            assert_eq!(
                all_log_links,
                vec![
                    (
                        "other_target.txt".into(),
                        format!("{build_url}/other_target.txt")
                    ),
                    (
                        "x86_64-unknown-linux-gnu.txt".into(),
                        format!("{build_url}/x86_64-unknown-linux-gnu.txt"),
                    )
                ]
            );

            for (url, expected_content) in &[
                (&all_log_links[0].1, "other target build log"),
                (&all_log_links[1].1, "A build log"),
            ] {
                let other_log = kuchikiki::parse_html()
                    .one(env.frontend().get(url).send()?.text()?)
                    .select("pre")
                    .unwrap()
                    .next()
                    .unwrap()
                    .text_contents();

                assert!(other_log.contains(expected_content));
            }

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
