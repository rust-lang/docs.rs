use crate::{
    db::types::BuildStatus,
    impl_axum_webpage,
    web::{
        error::{AxumNope, AxumResult},
        extractors::{DbConnection, Path},
        file::File,
        filters, MetaData,
    },
    AsyncStorage, Config,
};
use anyhow::Context as _;
use axum::{extract::Extension, response::IntoResponse};
use chrono::{DateTime, Utc};
use futures_util::TryStreamExt;
use rinja::Template;
use semver::Version;
use serde::Deserialize;
use std::sync::Arc;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct BuildDetails {
    id: i32,
    rustc_version: Option<String>,
    docsrs_version: Option<String>,
    build_status: BuildStatus,
    build_time: Option<DateTime<Utc>>,
    output: String,
    errors: Option<String>,
}

#[derive(Template)]
#[template(path = "crate/build_details.html")]
#[derive(Debug, Clone, PartialEq, Eq)]
struct BuildDetailsPage {
    metadata: MetaData,
    build_details: BuildDetails,
    all_log_filenames: Vec<String>,
    current_filename: Option<String>,
    csp_nonce: String,
}

impl_axum_webpage! { BuildDetailsPage }

// Used for template rendering.
impl BuildDetailsPage {
    pub(crate) fn get_metadata(&self) -> Option<&MetaData> {
        Some(&self.metadata)
    }
    pub(crate) fn use_direct_platform_links(&self) -> bool {
        true
    }
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
        r#"SELECT
             builds.rustc_version,
             builds.docsrs_version,
             builds.build_status as "build_status: BuildStatus",
             builds.build_time,
             builds.output,
             builds.errors,
             releases.default_target
         FROM builds
         INNER JOIN releases ON releases.id = builds.rid
         INNER JOIN crates ON releases.crate_id = crates.id
         WHERE builds.id = $1 AND crates.name = $2 AND releases.version = $3"#,
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

        if let Some(current_filename) = params
            .filename
            .or(row.default_target.map(|target| format!("{}.txt", target)))
        {
            let path = format!("{prefix}{current_filename}");
            let file = File::from_path(&storage, &path, &config).await?;
            (
                String::from_utf8(file.0.content).context("non utf8")?,
                storage
                    .list_prefix(&prefix) // the result from S3 is ordered by key
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
        } else {
            // this can only happen when `releases.default_target` is NULL,
            // which is the case for in-progress builds or builds which errored
            // before we could determine the target.
            // For the "error" case we show `row.errors`, which should contain what we need to see.
            ("".into(), Vec::new(), None)
        }
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
            errors: row.errors,
        },
        all_log_filenames,
        current_filename,
        csp_nonce: String::new(),
    }
    .into_response())
}

#[cfg(test)]
mod tests {
    use crate::test::{fake_release_that_failed_before_build, wrapper, FakeBuild};
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
    fn test_partial_build_result() {
        wrapper(|env| {
            let (_, build_id) = env.runtime().block_on(async {
                let mut conn = env.async_db().await.async_conn().await;
                fake_release_that_failed_before_build(
                    &mut conn,
                    "foo",
                    "0.1.0",
                    "some random error",
                )
                .await
            })?;

            let page = kuchikiki::parse_html().one(
                env.frontend()
                    .get(&format!("/crate/foo/0.1.0/builds/{build_id}"))
                    .send()?
                    .error_for_status()?
                    .text()?,
            );

            let info_text = page.select("pre").unwrap().next().unwrap().text_contents();

            assert!(info_text.contains("# pre-build errors"), "{}", info_text);
            assert!(info_text.contains("some random error"), "{}", info_text);

            Ok(())
        });
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
                    .error_for_status()?
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
