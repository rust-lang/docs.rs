use crate::web::{
    cache::CachePolicy,
    error::{AxumNope, AxumResult},
    extractors::{DbConnection, rustdoc::RustdocParams},
    match_version,
};
use axum::{
    Json, extract::Extension, http::header::ACCESS_CONTROL_ALLOW_ORIGIN, response::IntoResponse,
};

pub(crate) async fn status_handler(
    params: RustdocParams,
    mut conn: DbConnection,
) -> impl IntoResponse {
    (
        Extension(CachePolicy::NoStoreMustRevalidate),
        [(ACCESS_CONTROL_ALLOW_ORIGIN, "*")],
        // We use an async block to emulate a try block so that we can apply the above CORS header
        // and cache policy to both successful and failed responses
        async move {
            let matched_release = match_version(&mut conn, params.name(), params.req_version())
                .await?
                .assume_exact_name()?;

            let rustdoc_status = matched_release.rustdoc_status();

            let version = matched_release
                .into_canonical_req_version_or_else(|confirmed_name, version| {
                    AxumNope::Redirect(
                        params
                            .clone()
                            .with_name(confirmed_name)
                            .with_req_version(version)
                            .build_status_url(),
                        CachePolicy::NoCaching,
                    )
                })?
                .into_version();

            let json = Json(serde_json::json!({
                "version": version.to_string(),
                "doc_status": rustdoc_status,
            }));

            AxumResult::Ok(json.into_response())
        }
        .await,
    )
}

#[cfg(test)]
mod tests {
    use crate::{
        test::{AxumResponseTestExt, AxumRouterTestExt, async_wrapper},
        web::{ReqVersion, cache::CachePolicy},
    };
    use reqwest::StatusCode;
    use test_case::test_case;

    #[test_case("latest")]
    #[test_case("0.1")]
    #[test_case("0.1.0")]
    #[test_case("=0.1.0"; "exact_version")]
    fn status(req_version: &str) {
        async_wrapper(|env| async move {
            let req_version: ReqVersion = req_version.parse()?;

            env.fake_release()
                .await
                .name("foo")
                .version("0.1.0")
                .create()
                .await?;

            let response = env
                .web_app()
                .await
                .get_and_follow_redirects(&format!("/crate/foo/{req_version}/status.json"))
                .await?;
            response.assert_cache_control(CachePolicy::NoStoreMustRevalidate, env.config());
            assert_eq!(response.headers()["access-control-allow-origin"], "*");
            assert_eq!(response.status(), StatusCode::OK);
            let value: serde_json::Value = serde_json::from_str(&response.text().await?)?;

            assert_eq!(
                value,
                serde_json::json!({
                    "version": "0.1.0",
                    "doc_status": true,
                })
            );

            Ok(())
        });
    }

    #[test]
    fn redirect_latest() {
        async_wrapper(|env| async move {
            env.fake_release()
                .await
                .name("foo")
                .version("0.1.0")
                .create()
                .await?;

            let web = env.web_app().await;
            let redirect = web
                .assert_redirect("/crate/foo/*/status.json", "/crate/foo/latest/status.json")
                .await?;
            redirect.assert_cache_control(CachePolicy::NoStoreMustRevalidate, env.config());
            assert_eq!(redirect.headers()["access-control-allow-origin"], "*");

            Ok(())
        });
    }

    #[test_case("0.1")]
    #[test_case("~0.1"; "semver")]
    fn redirect(req_version: &str) {
        async_wrapper(|env| async move {
            let req_version: ReqVersion = req_version.parse()?;

            env.fake_release()
                .await
                .name("foo")
                .version("0.1.0")
                .create()
                .await?;

            let web = env.web_app().await;
            let redirect = web
                .assert_redirect(
                    &format!("/crate/foo/{req_version}/status.json"),
                    "/crate/foo/0.1.0/status.json",
                )
                .await?;
            redirect.assert_cache_control(CachePolicy::NoStoreMustRevalidate, env.config());
            assert_eq!(redirect.headers()["access-control-allow-origin"], "*");

            Ok(())
        });
    }

    #[test_case("latest")]
    #[test_case("0.1")]
    #[test_case("0.1.0")]
    #[test_case("=0.1.0"; "exact_version")]
    fn failure(req_version: &str) {
        async_wrapper(|env| async move {
            let req_version: ReqVersion = req_version.parse()?;

            env.fake_release()
                .await
                .name("foo")
                .version("0.1.0")
                .build_result_failed()
                .create()
                .await?;

            let response = env
                .web_app()
                .await
                .get_and_follow_redirects(&format!("/crate/foo/{req_version}/status.json"))
                .await?;
            response.assert_cache_control(CachePolicy::NoStoreMustRevalidate, env.config());
            assert_eq!(response.headers()["access-control-allow-origin"], "*");
            assert_eq!(response.status(), StatusCode::OK);
            let value: serde_json::Value = serde_json::from_str(&response.text().await?)?;

            assert_eq!(
                value,
                serde_json::json!({
                    "version": "0.1.0",
                    "doc_status": false,
                })
            );

            Ok(())
        });
    }

    // crate not found
    #[test_case("bar", "0.1")]
    #[test_case("bar", "0.1.0")]
    // version not found
    #[test_case("foo", "=0.1.0"; "exact_version")]
    #[test_case("foo", "0.2")]
    #[test_case("foo", "0.2.0")]
    // invalid semver
    #[test_case("foo", "0,1")]
    #[test_case("foo", "0,1,0")]
    fn not_found(krate: &str, req_version: &str) {
        async_wrapper(|env| async move {
            env.fake_release()
                .await
                .name("foo")
                .version("0.1.1")
                .create()
                .await?;

            let response = env
                .web_app()
                .await
                .get_and_follow_redirects(&format!("/crate/{krate}/{req_version}/status.json"))
                .await?;
            response.assert_cache_control(CachePolicy::NoStoreMustRevalidate, env.config());
            assert_eq!(response.headers()["access-control-allow-origin"], "*");
            assert_eq!(response.status(), StatusCode::NOT_FOUND);
            Ok(())
        });
    }
}
