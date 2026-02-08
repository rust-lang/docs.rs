use crate::{
    Config,
    cache::CachePolicy,
    error::{AxumNope, AxumResult, JsonAxumNope, JsonAxumResult},
    extractors::{DbConnection, Path, rustdoc::RustdocParams},
    impl_axum_webpage,
    match_release::match_version,
    metadata::MetaData,
    page::templates::{RenderBrands, RenderRegular, RenderSolid, filters},
};
use anyhow::{Result, anyhow};
use askama::Template;
use axum::{Json, extract::Extension, response::IntoResponse};
use axum_extra::{
    TypedHeader,
    headers::{Authorization, authorization::Bearer},
};
use chrono::{DateTime, Utc};
use constant_time_eq::constant_time_eq;
use docs_rs_build_limits::Limits;
use docs_rs_build_queue::{AsyncBuildQueue, PRIORITY_MANUAL_FROM_CRATES_IO};
use docs_rs_context::Context;
use docs_rs_headers::CanonicalUrl;
use docs_rs_types::{BuildId, BuildStatus, Duration, KrateName, ReqVersion, Version};
use http::StatusCode;
use std::sync::Arc;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Build {
    id: BuildId,
    rustc_version: Option<String>,
    docsrs_version: Option<String>,
    build_status: BuildStatus,
    build_time: Option<DateTime<Utc>>,
    build_duration: Option<Duration>,
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
    params: RustdocParams,
}

impl_axum_webpage! { BuildsPage }

impl BuildsPage {
    pub(crate) fn use_direct_platform_links(&self) -> bool {
        true
    }
}

pub(crate) async fn build_list_handler(
    params: RustdocParams,
    mut conn: DbConnection,
    Extension(context): Extension<Arc<Context>>,
) -> AxumResult<impl IntoResponse> {
    let version = match_version(&mut conn, params.name(), params.req_version())
        .await?
        .assume_exact_name()?
        .into_canonical_req_version_or_else(|confirmed_name, version| {
            let params = params
                .clone()
                .with_name(confirmed_name)
                .with_req_version(version);
            AxumNope::Redirect(
                params.builds_url(),
                CachePolicy::ForeverInCdn(confirmed_name.into()),
            )
        })?
        .into_version();

    let metadata = MetaData::from_crate(
        &mut conn,
        params.name(),
        &version,
        Some(params.req_version().clone()),
    )
    .await?;
    let params = params.apply_metadata(&metadata);

    Ok(BuildsPage {
        metadata,
        builds: get_builds(&mut conn, params.name(), &version).await?,
        limits: Limits::for_crate(context.config().build_limits()?, &mut conn, params.name())
            .await?,
        canonical_url: CanonicalUrl::from_uri(
            params
                .clone()
                .with_req_version(&ReqVersion::Latest)
                .builds_url(),
        ),
        params,
    }
    .into_response())
}

async fn crate_version_exists(
    conn: &mut sqlx::PgConnection,
    name: &KrateName,
    version: &Version,
) -> Result<bool, anyhow::Error> {
    let row = sqlx::query!(
        r#"
        SELECT 1 as "dummy"
        FROM releases
        INNER JOIN crates ON crates.id = releases.crate_id
        WHERE crates.name = $1 AND releases.version = $2
        LIMIT 1"#,
        name as _,
        version as _,
    )
    .fetch_optional(&mut *conn)
    .await?;
    Ok(row.is_some())
}

async fn build_trigger_check(
    conn: &mut sqlx::PgConnection,
    name: &KrateName,
    version: &Version,
    build_queue: &Arc<AsyncBuildQueue>,
) -> AxumResult<impl IntoResponse> {
    if !crate_version_exists(&mut *conn, name, version).await? {
        return Err(AxumNope::VersionNotFound);
    }

    let crate_version_is_in_queue = build_queue.has_build_queued(name, version).await?;

    if crate_version_is_in_queue {
        return Err(AxumNope::BadRequest(anyhow!(
            "crate {name} {version} already queued for rebuild"
        )));
    }

    Ok(())
}

pub(crate) async fn build_trigger_rebuild_handler(
    Path((name, version)): Path<(KrateName, Version)>,
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
            &version,
            PRIORITY_MANUAL_FROM_CRATES_IO,
            None, /* because crates.io is the only service that calls this endpoint */
        )
        .await
        .map_err(|e| JsonAxumNope(e.into()))?;

    Ok((StatusCode::CREATED, Json(serde_json::json!({}))))
}

async fn get_builds(
    conn: &mut sqlx::PgConnection,
    name: &KrateName,
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
            CASE
                WHEN builds.build_started IS NULL
                    -- for old builds, `build_started` is empty.
                    THEN NULL
                ELSE
                    CASE
                        -- for in-progress builds we show the duration until now
                        WHEN builds.build_status = 'in_progress' THEN (CURRENT_TIMESTAMP - builds.build_started)
                        -- there are broken builds where the status is `error`, and `build_finished` is NULL
                        WHEN builds.build_finished IS NULL THEN NULL
                        -- for finished builds we can show the full duration
                        ELSE (builds.build_finished - builds.build_started)
                    END
            END AS "build_duration?: Duration",
            builds.errors
         FROM builds
         INNER JOIN releases ON releases.id = builds.rid
         INNER JOIN crates ON releases.crate_id = crates.id
         WHERE
            crates.name = $1 AND
            releases.version = $2
         ORDER BY builds.id DESC"#,
        name as _,
        version as _,
    )
    .fetch_all(&mut *conn)
    .await?)
}

#[cfg(test)]
mod tests {
    use crate::{
        Config,
        cache::CachePolicy,
        testing::{
            AxumResponseTestExt, AxumRouterTestExt, TestEnvironment, TestEnvironmentExt as _,
            async_wrapper,
        },
    };
    use anyhow::Result;
    use axum::{body::Body, http::Request};
    use docs_rs_build_limits::Overrides;
    use docs_rs_test_fakes::{FakeBuild, fake_release_that_failed_before_build};
    use docs_rs_types::{
        BuildStatus,
        testing::{FOO, V1, V2},
    };
    use kuchikiki::traits::TendrilSink;
    use reqwest::StatusCode;
    use tower::ServiceExt;

    #[test]
    fn build_list_empty_build() {
        async_wrapper(|env| async move {
            let mut conn = env.async_conn().await?;
            fake_release_that_failed_before_build(&mut conn, "foo", "0.1.0", "some errors").await?;

            let response = env
                .web_app()
                .await
                .assert_success("/crate/foo/0.1.0/builds")
                .await?
                .error_for_status()?;
            response.assert_cache_control(CachePolicy::NoCaching, env.config());
            let page = kuchikiki::parse_html().one(response.text().await?);

            let rows: Vec<_> = page
                .select("ul > li a.release")
                .unwrap()
                .map(|row| row.text_contents())
                .collect();

            assert_eq!(rows.len(), 1);
            assert_eq!(rows[0].chars().filter(|&c| c == '—').count(), 3);

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
            response.assert_cache_control(CachePolicy::NoCaching, env.config());
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

    #[tokio::test(flavor = "multi_thread")]
    async fn build_trigger_rebuild_missing_config() -> Result<()> {
        let env = TestEnvironment::builder()
            .config(
                Config::builder()
                    .test_config()?
                    .maybe_cratesio_token(None)
                    .build(),
            )
            .build()
            .await?;

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
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn build_trigger_rebuild_with_config() -> Result<()> {
        let correct_token = "foo137";
        let env = TestEnvironment::builder()
            .config(
                Config::builder()
                    .test_config()?
                    .cratesio_token(correct_token.into())
                    .build(),
            )
            .build()
            .await?;

        env.fake_release()
            .await
            .name("foo")
            .version(V1)
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

        let build_queue = env.build_queue()?;

        assert_eq!(build_queue.pending_count().await?, 0);
        assert!(!build_queue.has_build_queued(&FOO, &V1).await?);

        {
            let app = env.web_app().await;
            let response = app
                .oneshot(
                    Request::builder()
                        .uri(format!("/crate/foo/{V1}/rebuild"))
                        .method("POST")
                        .header("Authorization", &format!("Bearer {correct_token}"))
                        .body(Body::empty())
                        .unwrap(),
                )
                .await?;
            assert_eq!(response.status(), StatusCode::CREATED);
            let json: serde_json::Value = response.json().await?;
            assert_eq!(json, serde_json::json!({}));
        }

        assert_eq!(build_queue.pending_count().await?, 1);
        assert!(build_queue.has_build_queued(&FOO, &V1).await?);

        {
            let app = env.web_app().await;
            let response = app
                .oneshot(
                    Request::builder()
                        .uri(format!("/crate/foo/{V1}/rebuild"))
                        .method("POST")
                        .header("Authorization", &format!("Bearer {correct_token}"))
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
                    "message": format!("crate foo {V1} already queued for rebuild")
                })
            );
        }

        assert_eq!(build_queue.pending_count().await?, 1);
        assert!(build_queue.has_build_queued(&FOO, &V1).await?);

        Ok(())
    }

    #[test]
    fn build_empty_list() {
        async_wrapper(|env| async move {
            env.fake_release()
                .await
                .name("foo")
                .version(V1)
                .no_builds()
                .create()
                .await?;

            let response = env
                .web_app()
                .await
                .get(&format!("/crate/foo/{V1}/builds"))
                .await?;

            response.assert_cache_control(CachePolicy::NoCaching, env.config());
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
                .version(V1)
                .create()
                .await?;

            let mut conn = env.async_conn().await?;
            let limits = Overrides {
                memory: Some(6 * 1024 * 1024 * 1024),
                targets: Some(1),
                timeout: Some(std::time::Duration::from_secs(2 * 60 * 60)),
            };
            Overrides::save(&mut conn, &FOO, limits).await?;

            let page = kuchikiki::parse_html().one(dbg!(
                env.web_app()
                    .await
                    .assert_success(&format!("/crate/foo/{V1}/builds"))
                    .await?
                    .text()
                    .await?
            ));

            let header = page.select(".about h4").unwrap().next().unwrap();
            assert_eq!(header.text_contents(), "foo's sandbox limits");

            let values: Vec<_> = page
                .select(".about table tr td:last-child")
                .unwrap()
                .map(|row| row.text_contents())
                .collect();
            let values: Vec<_> = values.iter().map(|v| &**v).collect();

            assert!(values.contains(&"6.44 GB"));
            assert!(values.contains(&"2h"));
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
                .version(V1)
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
                .version(V2)
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
                .assert_success("/crate/aquarelle/latest/status.json")
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
                .version(V1)
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
                .get(&format!("/crate/foo/{V2}/builds"))
                .await?;
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
