use crate::{
    AsyncStorage, Config,
    db::{BuildId, types::BuildStatus},
    impl_axum_webpage,
    web::{
        MetaData,
        error::{AxumNope, AxumResult},
        extractors::{DbConnection, Path},
        file::File,
        filters,
        page::templates::{RenderRegular, RenderSolid},
    },
};
use anyhow::Context as _;
use askama::Template;
use axum::{extract::Extension, response::IntoResponse};
use chrono::{DateTime, Utc};
use futures_util::TryStreamExt;
use semver::Version;
use serde::Deserialize;
use std::sync::Arc;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct BuildDetails {
    id: BuildId,
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
    let id = params
        .id
        .parse()
        .map(BuildId)
        .map_err(|_| AxumNope::BuildNotFound)?;

    let row = sqlx::query!(
        r#"SELECT
             builds.rustc_version,
             builds.docsrs_version,
             builds.build_status as "build_status: BuildStatus",
             COALESCE(builds.build_finished, builds.build_started) as build_time,
             builds.output,
             builds.errors,
             releases.default_target
         FROM builds
         INNER JOIN releases ON releases.id = builds.rid
         INNER JOIN crates ON releases.crate_id = crates.id
         WHERE builds.id = $1 AND crates.name = $2 AND releases.version = $3"#,
        id.0,
        params.name,
        params.version.to_string(),
    )
    .fetch_optional(&mut *conn)
    .await?
    .ok_or(AxumNope::BuildNotFound)?;

    let (output, all_log_filenames, current_filename) = if let Some(output) = row.output {
        // legacy case, for old builds the build log was stored in the database.
        (output, Vec::new(), None)
    } else {
        // for newer builds we have the build logs stored in S3.
        // For a long time only for one target, then we started storing the logs for other targets
        // toFor a long time only for one target, then we started storing the logs for other
        // targets. In any case, all the logfiles are put into a folder we can just query.
        let prefix = format!("build-logs/{}/", id);
        let all_log_filenames: Vec<_> = storage
            .list_prefix(&prefix) // the result from S3 is ordered by key
            .await
            .map_ok(|path| {
                path.strip_prefix(&prefix)
                    .expect("since we query for the prefix, it has to be always there")
                    .to_owned()
            })
            .try_collect()
            .await?;

        let current_filename = if let Some(filename) = params.filename {
            // if we have a given filename in the URL, we use that one.
            Some(filename)
        } else if let Some(default_target) = row.default_target {
            // without a filename in the URL, we try to show the build log
            // for the default target, if we have one.
            let wanted_filename = format!("{default_target}.txt");
            if all_log_filenames.contains(&wanted_filename) {
                Some(wanted_filename)
            } else {
                None
            }
        } else {
            // this can only happen when `releases.default_target` is NULL,
            // which is the case for in-progress builds or builds which errored
            // before we could determine the target.
            // For the "error" case we show `row.errors`, which should contain what we need to see.
            None
        };

        let file_content = if let Some(ref filename) = current_filename {
            let file = File::from_path(&storage, &format!("{prefix}{filename}"), &config).await?;
            String::from_utf8(file.0.content).context("non utf8")?
        } else {
            "".to_string()
        };

        (file_content, all_log_filenames, current_filename)
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
    use crate::test::{
        AxumResponseTestExt, AxumRouterTestExt, FakeBuild, async_wrapper,
        fake_release_that_failed_before_build,
    };
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
        async_wrapper(|env| async move {
            let mut conn = env.async_db().await.async_conn().await;
            let (_, build_id) = fake_release_that_failed_before_build(
                &mut conn,
                "foo",
                "0.1.0",
                "some random error",
            )
            .await?;

            let page = kuchikiki::parse_html().one(
                env.web_app()
                    .await
                    .get(&format!("/crate/foo/0.1.0/builds/{build_id}"))
                    .await?
                    .error_for_status()?
                    .text()
                    .await?,
            );

            let info_text = page.select("pre").unwrap().next().unwrap().text_contents();

            assert!(info_text.contains("# pre-build errors"), "{}", info_text);
            assert!(info_text.contains("some random error"), "{}", info_text);

            Ok(())
        });
    }

    #[test]
    fn test_partial_build_result_plus_default_target_from_previous_build() {
        async_wrapper(|env| async move {
            let mut conn = env.async_db().await.async_conn().await;
            let (release_id, build_id) = fake_release_that_failed_before_build(
                &mut conn,
                "foo",
                "0.1.0",
                "some random error",
            )
            .await?;

            sqlx::query!(
                "UPDATE releases SET default_target = 'x86_64-unknown-linux-gnu' WHERE id = $1",
                release_id.0
            )
            .execute(&mut *conn)
            .await?;

            let page = kuchikiki::parse_html().one(
                env.web_app()
                    .await
                    .get(&format!("/crate/foo/0.1.0/builds/{build_id}"))
                    .await?
                    .error_for_status()?
                    .text()
                    .await?,
            );

            let info_text = page.select("pre").unwrap().next().unwrap().text_contents();

            assert!(info_text.contains("# pre-build errors"), "{}", info_text);
            assert!(info_text.contains("some random error"), "{}", info_text);

            Ok(())
        });
    }

    #[test]
    fn db_build_logs() {
        async_wrapper(|env| async move {
            env.fake_release()
                .await
                .name("foo")
                .version("0.1.0")
                .builds(vec![
                    FakeBuild::default()
                        .no_s3_build_log()
                        .db_build_log("A build log"),
                ])
                .create()
                .await?;

            let web = env.web_app().await;

            let page = kuchikiki::parse_html().one(
                web.get("/crate/foo/0.1.0/builds")
                    .await?
                    .error_for_status()?
                    .text()
                    .await?,
            );

            let node = page.select("ul > li a.release").unwrap().next().unwrap();
            let url = {
                let attrs = node.attributes.borrow();
                attrs.get("href").unwrap().to_owned()
            };

            let page = kuchikiki::parse_html().one(web.get(&url).await?.text().await?);
            assert!(get_all_log_links(&page).is_empty());

            let log = page.select("pre").unwrap().next().unwrap().text_contents();

            assert!(log.contains("A build log"));

            Ok(())
        });
    }

    #[test]
    fn s3_build_logs() {
        async_wrapper(|env| async move {
            env.fake_release()
                .await
                .name("foo")
                .version("0.1.0")
                .builds(vec![FakeBuild::default().s3_build_log("A build log")])
                .create()
                .await?;

            let web = env.web_app().await;

            let page = kuchikiki::parse_html()
                .one(web.get("/crate/foo/0.1.0/builds").await?.text().await?);

            let node = page.select("ul > li a.release").unwrap().next().unwrap();
            let build_url = {
                let attrs = node.attributes.borrow();
                attrs.get("href").unwrap().to_owned()
            };

            let page = kuchikiki::parse_html().one(web.get(&build_url).await?.text().await?);

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
                .one(web.get(&all_log_links[0].1).await?.text().await?)
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
        async_wrapper(|env| async move {
            env.fake_release()
                .await
                .name("foo")
                .version("0.1.0")
                .builds(vec![
                    FakeBuild::default()
                        .s3_build_log("A build log")
                        .build_log_for_other_target("other_target", "other target build log"),
                ])
                .create()
                .await?;

            let web = env.web_app().await;

            let page = kuchikiki::parse_html()
                .one(web.get("/crate/foo/0.1.0/builds").await?.text().await?);

            let node = page.select("ul > li a.release").unwrap().next().unwrap();
            let build_url = {
                let attrs = node.attributes.borrow();
                attrs.get("href").unwrap().to_owned()
            };

            let page = kuchikiki::parse_html().one(web.get(&build_url).await?.text().await?);

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
                    .one(web.get(url).await?.text().await?)
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
        async_wrapper(|env| async move {
            env.fake_release()
                .await
                .name("foo")
                .version("0.1.0")
                .builds(vec![
                    FakeBuild::default()
                        .s3_build_log("A build log")
                        .db_build_log("Another build log"),
                ])
                .create()
                .await?;

            let web = env.web_app().await;

            let page = kuchikiki::parse_html()
                .one(web.get("/crate/foo/0.1.0/builds").await?.text().await?);

            let node = page.select("ul > li a.release").unwrap().next().unwrap();
            let url = {
                let attrs = node.attributes.borrow();
                attrs.get("href").unwrap().to_owned()
            };

            let page = kuchikiki::parse_html().one(web.get(&url).await?.text().await?);

            let log = page.select("pre").unwrap().next().unwrap().text_contents();

            // Relatively arbitrarily the DB is prioritised
            assert!(log.contains("Another build log"));

            Ok(())
        });
    }

    #[test_case("42")]
    #[test_case("nan")]
    fn non_existing_build(build_id: &str) {
        async_wrapper(|env| async move {
            env.fake_release()
                .await
                .name("foo")
                .version("0.1.0")
                .create()
                .await?;

            let res = env
                .web_app()
                .await
                .get(&format!("/crate/foo/0.1.0/builds/{build_id}"))
                .await?;
            assert_eq!(res.status(), 404);
            assert!(res.text().await?.contains("no such build"));

            Ok(())
        });
    }
}
