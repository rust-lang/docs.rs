use super::{
    cache::CachePolicy,
    error::{AxumNope, JsonAxumNope, JsonAxumResult},
    headers::CanonicalUrl,
};
use crate::{
    AsyncBuildQueue, Config,
    db::{BuildId, types::BuildStatus},
    docbuilder::Limits,
    impl_axum_webpage,
    web::{
        MetaData, ReqVersion,
        error::{AxumResult, EscapedURI},
        extractors::{DbConnection, Path},
        filters, match_version,
        page::templates::{RenderRegular, RenderSolid},
    },
};
use anyhow::{Result, anyhow};
use askama::Template;
use axum::{
    Json, extract::Extension, http::header::ACCESS_CONTROL_ALLOW_ORIGIN, response::IntoResponse,
};
use axum_extra::{
    TypedHeader,
    headers::{Authorization, authorization::Bearer},
};
use chrono::{DateTime, Utc};
use constant_time_eq::constant_time_eq;
use http::StatusCode;
use semver::Version;
use std::sync::Arc;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Build {
    id: BuildId,
    rustc_version: Option<String>,
    docsrs_version: Option<String>,
    build_status: BuildStatus,
    build_time: Option<DateTime<Utc>>,
    errors: Option<String>,
}

#[derive(Template)]
#[template(path = "crate/builds.html")]
#[derive(Debug, Clone)]
struct BuildsPage {
    metadata: MetaData,
    builds: Vec<Build>,
    limits: Limits,
    canonical_url: CanonicalUrl,
}

impl_axum_webpage! { BuildsPage }

impl BuildsPage {
    pub(crate) fn use_direct_platform_links(&self) -> bool {
        true
    }
}

pub(crate) async fn build_list_handler(
    Path((name, req_version)): Path<(String, ReqVersion)>,
    mut conn: DbConnection,
    Extension(config): Extension<Arc<Config>>,
) -> AxumResult<impl IntoResponse> {
    let version = match_version(&mut conn, &name, &req_version)
        .await?
        .assume_exact_name()?
        .into_canonical_req_version_or_else(|version| {
            AxumNope::Redirect(
                EscapedURI::new(&format!("/crate/{name}/{version}/builds"), None),
                CachePolicy::ForeverInCdn,
            )
        })?
        .into_version();

    Ok(BuildsPage {
        metadata: MetaData::from_crate(&mut conn, &name, &version, Some(req_version)).await?,
        builds: get_builds(&mut conn, &name, &version).await?,
        limits: Limits::for_crate(&config, &mut conn, &name).await?,
        canonical_url: CanonicalUrl::from_path(format!("/crate/{name}/latest/builds")),
    }
    .into_response())
}

pub(crate) async fn build_list_json_handler(
    Path((name, req_version)): Path<(String, ReqVersion)>,
    mut conn: DbConnection,
) -> AxumResult<impl IntoResponse> {
    let version = match_version(&mut conn, &name, &req_version)
        .await?
        .assume_exact_name()?
        .into_canonical_req_version_or_else(|version| {
            AxumNope::Redirect(
                EscapedURI::new(&format!("/crate/{name}/{version}/builds.json"), None),
                CachePolicy::ForeverInCdn,
            )
        })?
        .into_version();

    Ok((
        Extension(CachePolicy::NoStoreMustRevalidate),
        [(ACCESS_CONTROL_ALLOW_ORIGIN, "*")],
        Json(
            get_builds(&mut conn, &name, &version)
                .await?
                .iter()
                .filter_map(|build| {
                    if build.build_status == BuildStatus::InProgress {
                        return None;
                    }
                    // for backwards compatibility in this API, we
                    // * convert the build status to a boolean
                    // * already filter out in-progress builds
                    //
                    // even when we start showing in-progress builds in the UI,
                    // we might still not show them here for backwards
                    // compatibility.
                    Some(serde_json::json!({
                        "id": build.id,
                        "rustc_version": build.rustc_version,
                        "docsrs_version": build.docsrs_version,
                        "build_status": build.build_status.is_success(),
                        "build_time": build.build_time,
                    }))
                })
                .collect::<Vec<_>>(),
        ),
    )
        .into_response())
}

async fn crate_version_exists(
    conn: &mut sqlx::PgConnection,
    name: &String,
    version: &Version,
) -> Result<bool, anyhow::Error> {
    let row = sqlx::query!(
        r#"
        SELECT 1 as "dummy"
        FROM releases
        INNER JOIN crates ON crates.id = releases.crate_id
        WHERE crates.name = $1 AND releases.version = $2
        LIMIT 1"#,
        name,
        version.to_string(),
    )
    .fetch_optional(&mut *conn)
    .await?;
    Ok(row.is_some())
}

async fn build_trigger_check(
    conn: &mut sqlx::PgConnection,
    name: &String,
    version: &Version,
    build_queue: &Arc<AsyncBuildQueue>,
) -> AxumResult<impl IntoResponse> {
    if !crate_version_exists(&mut *conn, name, version).await? {
        return Err(AxumNope::VersionNotFound);
    }

    let crate_version_is_in_queue = build_queue
        .has_build_queued(name, &version.to_string())
        .await?;

    if crate_version_is_in_queue {
        return Err(AxumNope::BadRequest(anyhow!(
            "crate {name} {version} already queued for rebuild"
        )));
    }

    Ok(())
}

// Priority according to issue #2442; positive here as it's inverted.
// FUTURE: move to a crate-global enum with all special priorities?
const TRIGGERED_REBUILD_PRIORITY: i32 = 5;

pub(crate) async fn build_trigger_rebuild_handler(
    Path((name, version)): Path<(String, Version)>,
    mut conn: DbConnection,
    Extension(build_queue): Extension<Arc<AsyncBuildQueue>>,
    Extension(config): Extension<Arc<Config>>,
    opt_auth_header: Option<TypedHeader<Authorization<Bearer>>>,
) -> JsonAxumResult<impl IntoResponse> {
    let expected_token =
        config
            .cratesio_token
            .as_ref()
            .ok_or(JsonAxumNope(AxumNope::Unauthorized(
                "Endpoint is not configured",
            )))?;

    // (Future: would it be better to have standard middleware handle auth?)
    let TypedHeader(auth_header) = opt_auth_header.ok_or(JsonAxumNope(AxumNope::Unauthorized(
        "Missing authentication token",
    )))?;
    if !constant_time_eq(auth_header.token().as_bytes(), expected_token.as_bytes()) {
        return Err(JsonAxumNope(AxumNope::Unauthorized(
            "The token used for authentication is not valid",
        )));
    }

    build_trigger_check(&mut conn, &name, &version, &build_queue)
        .await
        .map_err(JsonAxumNope)?;

    build_queue
        .add_crate(
            &name,
            &version.to_string(),
            TRIGGERED_REBUILD_PRIORITY,
            None, /* because crates.io is the only service that calls this endpoint */
        )
        .await
        .map_err(|e| JsonAxumNope(e.into()))?;

    Ok((StatusCode::CREATED, Json(serde_json::json!({}))))
}

async fn get_builds(
    conn: &mut sqlx::PgConnection,
    name: &str,
    version: &Version,
) -> Result<Vec<Build>> {
    Ok(sqlx::query_as!(
        Build,
        r#"SELECT
            builds.id as "id: BuildId",
            builds.rustc_version,
            builds.docsrs_version,
            builds.build_status as "build_status: BuildStatus",
            COALESCE(builds.build_finished, builds.build_started) as build_time,
            builds.errors
         FROM builds
         INNER JOIN releases ON releases.id = builds.rid
         INNER JOIN crates ON releases.crate_id = crates.id
         WHERE
            crates.name = $1 AND
            releases.version = $2
         ORDER BY builds.id DESC"#,
        name,
        version.to_string(),
    )
    .fetch_all(&mut *conn)
    .await?)
}

#[cfg(test)]
mod tests {
    use super::BuildStatus;
    use crate::{
        db::Overrides,
        test::{
            AxumResponseTestExt, AxumRouterTestExt, FakeBuild, async_wrapper,
            fake_release_that_failed_before_build,
        },
        web::cache::CachePolicy,
    };
    use axum::{body::Body, http::Request};
    use chrono::{DateTime, Utc};
    use kuchikiki::traits::TendrilSink;
    use reqwest::StatusCode;
    use tower::ServiceExt;

    #[test]
    fn build_list_empty_build() {
        async_wrapper(|env| async move {
            let mut conn = env.async_db().await.async_conn().await;
            fake_release_that_failed_before_build(&mut conn, "foo", "0.1.0", "some errors").await?;

            let response = env
                .web_app()
                .await
                .get("/crate/foo/0.1.0/builds")
                .await?
                .error_for_status()?;
            response.assert_cache_control(CachePolicy::NoCaching, &env.config());
            let page = kuchikiki::parse_html().one(response.text().await?);

            let rows: Vec<_> = page
                .select("ul > li a.release")
                .unwrap()
                .map(|row| row.text_contents())
                .collect();

            assert_eq!(rows.len(), 1);
            // third column contains build-start time, even when the rest is empty
            assert_eq!(rows[0].chars().filter(|&c| c == '—').count(), 2);

            Ok(())
        });
    }

    #[test]
    fn build_list() {
        async_wrapper(|env| async move {
            env.fake_release()
                .await
                .name("foo")
                .version("0.1.0")
                .builds(vec![
                    FakeBuild::default()
                        .rustc_version("rustc (blabla 2019-01-01)")
                        .docsrs_version("docs.rs 1.0.0"),
                    FakeBuild::default()
                        .successful(false)
                        .rustc_version("rustc (blabla 2020-01-01)")
                        .docsrs_version("docs.rs 2.0.0"),
                    FakeBuild::default()
                        .rustc_version("rustc (blabla 2021-01-01)")
                        .docsrs_version("docs.rs 3.0.0"),
                    FakeBuild::default()
                        .build_status(BuildStatus::InProgress)
                        .rustc_version("rustc (blabla 2022-01-01)")
                        .docsrs_version("docs.rs 4.0.0"),
                ])
                .create()
                .await?;

            let response = env.web_app().await.get("/crate/foo/0.1.0/builds").await?;
            response.assert_cache_control(CachePolicy::NoCaching, &env.config());
            let page = kuchikiki::parse_html().one(response.text().await?);

            let rows: Vec<_> = page
                .select("ul > li a.release")
                .unwrap()
                .map(|row| row.text_contents())
                .collect();

            assert!(rows[0].contains("rustc (blabla 2021-01-01)"));
            assert!(rows[0].contains("docs.rs 3.0.0"));
            assert!(rows[1].contains("rustc (blabla 2020-01-01)"));
            assert!(rows[1].contains("docs.rs 2.0.0"));
            assert!(rows[2].contains("rustc (blabla 2019-01-01)"));
            assert!(rows[2].contains("docs.rs 1.0.0"));

            Ok(())
        });
    }

    #[test]
    fn build_list_json() {
        async_wrapper(|env| async move {
            env.fake_release()
                .await
                .name("foo")
                .version("0.1.0")
                .builds(vec![
                    FakeBuild::default()
                        .rustc_version("rustc (blabla 2019-01-01)")
                        .docsrs_version("docs.rs 1.0.0"),
                    FakeBuild::default()
                        .successful(false)
                        .rustc_version("rustc (blabla 2020-01-01)")
                        .docsrs_version("docs.rs 2.0.0"),
                    FakeBuild::default()
                        .rustc_version("rustc (blabla 2021-01-01)")
                        .docsrs_version("docs.rs 3.0.0"),
                    FakeBuild::default()
                        .build_status(BuildStatus::InProgress)
                        .rustc_version("rustc (blabla 2022-01-01)")
                        .docsrs_version("docs.rs 4.0.0"),
                ])
                .create()
                .await?;

            let response = env
                .web_app()
                .await
                .get("/crate/foo/0.1.0/builds.json")
                .await?;
            response.assert_cache_control(CachePolicy::NoStoreMustRevalidate, &env.config());
            let value: serde_json::Value = serde_json::from_str(&response.text().await?)?;

            assert_eq!(value.as_array().unwrap().len(), 3);

            assert_eq!(value.pointer("/0/build_status"), Some(&true.into()));
            assert_eq!(
                value.pointer("/0/docsrs_version"),
                Some(&"docs.rs 3.0.0".into())
            );
            assert_eq!(
                value.pointer("/0/rustc_version"),
                Some(&"rustc (blabla 2021-01-01)".into())
            );
            assert!(value.pointer("/0/id").unwrap().is_i64());
            assert!(
                serde_json::from_value::<DateTime<Utc>>(
                    value.pointer("/0/build_time").unwrap().clone()
                )
                .is_ok()
            );

            assert_eq!(value.pointer("/1/build_status"), Some(&false.into()));
            assert_eq!(
                value.pointer("/1/docsrs_version"),
                Some(&"docs.rs 2.0.0".into())
            );
            assert_eq!(
                value.pointer("/1/rustc_version"),
                Some(&"rustc (blabla 2020-01-01)".into())
            );
            assert!(value.pointer("/1/id").unwrap().is_i64());
            assert!(
                serde_json::from_value::<DateTime<Utc>>(
                    value.pointer("/1/build_time").unwrap().clone()
                )
                .is_ok()
            );

            assert_eq!(value.pointer("/2/build_status"), Some(&true.into()));
            assert_eq!(
                value.pointer("/2/docsrs_version"),
                Some(&"docs.rs 1.0.0".into())
            );
            assert_eq!(
                value.pointer("/2/rustc_version"),
                Some(&"rustc (blabla 2019-01-01)".into())
            );
            assert!(value.pointer("/2/id").unwrap().is_i64());
            assert!(
                serde_json::from_value::<DateTime<Utc>>(
                    value.pointer("/2/build_time").unwrap().clone()
                )
                .is_ok()
            );

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
    fn build_trigger_rebuild_missing_config() {
        async_wrapper(|env| async move {
            env.override_config(|config| config.cratesio_token = None);
            env.fake_release()
                .await
                .name("foo")
                .version("0.1.0")
                .create()
                .await?;

            {
                let response = env
                    .web_app()
                    .await
                    .get("/crate/regex/1.3.1/rebuild")
                    .await?;
                // Needs POST
                assert_eq!(response.status(), StatusCode::METHOD_NOT_ALLOWED);
            }

            {
                let response = env
                    .web_app()
                    .await
                    .post("/crate/regex/1.3.1/rebuild")
                    .await?;
                assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
                let json: serde_json::Value = response.json().await?;
                assert_eq!(
                    json,
                    serde_json::json!({
                        "title": "Unauthorized",
                        "message": "Endpoint is not configured"
                    })
                );
            }

            Ok(())
        })
    }

    #[test]
    fn build_trigger_rebuild_with_config() {
        async_wrapper(|env| async move {
            let correct_token = "foo137";
            env.override_config(|config| config.cratesio_token = Some(correct_token.into()));

            env.fake_release()
                .await
                .name("foo")
                .version("0.1.0")
                .create()
                .await?;

            {
                let response = env
                    .web_app()
                    .await
                    .post("/crate/regex/1.3.1/rebuild")
                    .await?;
                assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
                let json: serde_json::Value = response.json().await?;
                assert_eq!(
                    json,
                    serde_json::json!({
                        "title": "Unauthorized",
                        "message": "Missing authentication token"
                    })
                );
            }

            {
                let app = env.web_app().await;
                let response = app
                    .oneshot(
                        Request::builder()
                            .uri("/crate/regex/1.3.1/rebuild")
                            .method("POST")
                            .header("Authorization", "Bearer someinvalidtoken")
                            .body(Body::empty())
                            .unwrap(),
                    )
                    .await?;
                assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
                let json: serde_json::Value = response.json().await?;
                assert_eq!(
                    json,
                    serde_json::json!({
                        "title": "Unauthorized",
                        "message": "The token used for authentication is not valid"
                    })
                );
            }

            let build_queue = env.async_build_queue().await;

            assert_eq!(build_queue.pending_count().await?, 0);
            assert!(!build_queue.has_build_queued("foo", "0.1.0").await?);

            {
                let app = env.web_app().await;
                let response = app
                    .oneshot(
                        Request::builder()
                            .uri("/crate/foo/0.1.0/rebuild")
                            .method("POST")
                            .header("Authorization", &format!("Bearer {}", correct_token))
                            .body(Body::empty())
                            .unwrap(),
                    )
                    .await?;
                assert_eq!(response.status(), StatusCode::CREATED);
                let json: serde_json::Value = response.json().await?;
                assert_eq!(json, serde_json::json!({}));
            }

            assert_eq!(build_queue.pending_count().await?, 1);
            assert!(build_queue.has_build_queued("foo", "0.1.0").await?);

            {
                let app = env.web_app().await;
                let response = app
                    .oneshot(
                        Request::builder()
                            .uri("/crate/foo/0.1.0/rebuild")
                            .method("POST")
                            .header("Authorization", &format!("Bearer {}", correct_token))
                            .body(Body::empty())
                            .unwrap(),
                    )
                    .await?;
                assert_eq!(response.status(), StatusCode::BAD_REQUEST);
                let json: serde_json::Value = response.json().await?;
                assert_eq!(
                    json,
                    serde_json::json!({
                        "title": "Bad request",
                        "message": "crate foo 0.1.0 already queued for rebuild"
                    })
                );
            }

            assert_eq!(build_queue.pending_count().await?, 1);
            assert!(build_queue.has_build_queued("foo", "0.1.0").await?);

            Ok(())
        });
    }

    #[test]
    fn build_empty_list() {
        async_wrapper(|env| async move {
            env.fake_release()
                .await
                .name("foo")
                .version("0.1.0")
                .no_builds()
                .create()
                .await?;

            let response = env.web_app().await.get("/crate/foo/0.1.0/builds").await?;

            response.assert_cache_control(CachePolicy::NoCaching, &env.config());
            let page = kuchikiki::parse_html().one(response.text().await?);

            let rows: Vec<_> = page
                .select("ul > li a.release")
                .unwrap()
                .map(|row| row.text_contents())
                .collect();

            assert!(rows.is_empty());

            let warning = page
                .select_first(".warning")
                .expect("missing warning element")
                .text_contents();

            assert!(warning.contains("has not built"));
            assert!(warning.contains("queued"));
            assert!(warning.contains("open an issue"));

            Ok(())
        });
    }

    #[test]
    fn limits() {
        async_wrapper(|env| async move {
            env.fake_release()
                .await
                .name("foo")
                .version("0.1.0")
                .create()
                .await?;

            let mut conn = env.async_db().await.async_conn().await;
            let limits = Overrides {
                memory: Some(6 * 1024 * 1024 * 1024),
                targets: Some(1),
                timeout: Some(std::time::Duration::from_secs(2 * 60 * 60)),
            };
            Overrides::save(&mut conn, "foo", limits).await?;

            let page = kuchikiki::parse_html().one(
                env.web_app()
                    .await
                    .get("/crate/foo/0.1.0/builds")
                    .await?
                    .text()
                    .await?,
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
            assert!(values.contains(&"6.44 GB"));
            assert!(values.contains(&"2 hours"));
            assert!(values.contains(&"102.4 kB"));
            assert!(values.contains(&"blocked"));
            assert!(values.contains(&"1"));

            Ok(())
        });
    }

    #[test]
    fn latest_200() {
        async_wrapper(|env| async move {
            env.fake_release()
                .await
                .name("aquarelle")
                .version("0.1.0")
                .builds(vec![
                    FakeBuild::default()
                        .rustc_version("rustc (blabla 2019-01-01)")
                        .docsrs_version("docs.rs 1.0.0"),
                ])
                .create()
                .await?;

            env.fake_release()
                .await
                .name("aquarelle")
                .version("0.2.0")
                .builds(vec![
                    FakeBuild::default()
                        .rustc_version("rustc (blabla 2019-01-01)")
                        .docsrs_version("docs.rs 1.0.0"),
                ])
                .create()
                .await?;

            let resp = env
                .web_app()
                .await
                .get("/crate/aquarelle/latest/builds")
                .await?;
            let body = resp.text().await?;
            assert!(body.contains("<a href=\"/crate/aquarelle/latest/features\""));
            assert!(body.contains("<a href=\"/crate/aquarelle/latest/builds\""));
            assert!(body.contains("<a href=\"/crate/aquarelle/latest/source/\""));
            assert!(body.contains("<a href=\"/crate/aquarelle/latest\""));

            env.web_app()
                .await
                .assert_success("/crate/aquarelle/latest/builds.json")
                .await?;

            Ok(())
        });
    }

    #[test]
    fn crate_version_not_found() {
        async_wrapper(|env| async move {
            env.fake_release()
                .await
                .name("foo")
                .version("0.1.0")
                .builds(vec![
                    FakeBuild::default()
                        .rustc_version("rustc (blabla 2019-01-01)")
                        .docsrs_version("docs.rs 1.0.0"),
                ])
                .create()
                .await?;

            let resp = env.web_app().await.get("/crate/foo/0.2.0/builds").await?;
            assert_eq!(resp.status(), StatusCode::NOT_FOUND);
            Ok(())
        });
    }

    #[test]
    fn invalid_semver() {
        async_wrapper(|env| async move {
            env.fake_release()
                .await
                .name("foo")
                .version("0.1.0")
                .builds(vec![
                    FakeBuild::default()
                        .rustc_version("rustc (blabla 2019-01-01)")
                        .docsrs_version("docs.rs 1.0.0"),
                ])
                .create()
                .await?;

            let resp = env.web_app().await.get("/crate/foo/0,1,0/builds").await?;
            assert_eq!(resp.status(), StatusCode::NOT_FOUND);
            Ok(())
        });
    }
}
