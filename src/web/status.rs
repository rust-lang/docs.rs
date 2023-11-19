use super::cache::CachePolicy;
use crate::web::{
    axum_redirect, error::AxumResult, extractors::DbConnection, match_version, MatchSemver,
};
use axum::{
    extract::{Extension, Path},
    http::header::ACCESS_CONTROL_ALLOW_ORIGIN,
    response::IntoResponse,
    Json,
};

pub(crate) async fn status_handler(
    Path((name, req_version)): Path<(String, String)>,
    mut conn: DbConnection,
) -> impl IntoResponse {
    (
        Extension(CachePolicy::NoStoreMustRevalidate),
        [(ACCESS_CONTROL_ALLOW_ORIGIN, "*")],
        // We use an async block to emulate a try block so that we can apply the above CORS header
        // and cache policy to both successful and failed responses
        async move {
            let (version, id) = match match_version(&mut conn, &name, Some(&req_version))
                .await?
                .exact_name_only()?
            {
                MatchSemver::Exact((version, id)) | MatchSemver::Latest((version, id)) => {
                    (version, id)
                }
                MatchSemver::Semver((version, _)) => {
                    let redirect = axum_redirect(format!("/crate/{name}/{version}/status.json"))?;
                    return Ok(redirect.into_response());
                }
            };

            let rustdoc_status: bool = sqlx::query_scalar!(
                "SELECT releases.rustdoc_status
                 FROM releases
                 WHERE releases.id = $1
                ",
                id
            )
            .fetch_one(&mut *conn)
            .await?;

            let json = Json(serde_json::json!({
                "version": version,
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
        test::{assert_cache_control, assert_redirect, wrapper},
        web::cache::CachePolicy,
    };
    use reqwest::StatusCode;
    use test_case::test_case;

    #[test_case("latest")]
    #[test_case("0.1")]
    #[test_case("0.1.0")]
    #[test_case("=0.1.0"; "exact_version")]
    fn status(version: &str) {
        wrapper(|env| {
            env.fake_release().name("foo").version("0.1.0").create()?;

            let response = env
                .frontend()
                .get(&format!("/crate/foo/{version}/status.json"))
                .send()?;
            assert_cache_control(&response, CachePolicy::NoStoreMustRevalidate, &env.config());
            assert_eq!(response.headers()["access-control-allow-origin"], "*");
            assert_eq!(response.status(), StatusCode::OK);
            let value: serde_json::Value = serde_json::from_str(&response.text()?)?;

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

    #[test_case("0.1")]
    #[test_case("*")]
    fn redirect(version: &str) {
        wrapper(|env| {
            env.fake_release().name("foo").version("0.1.0").create()?;

            let redirect = assert_redirect(
                &format!("/crate/foo/{version}/status.json"),
                "/crate/foo/0.1.0/status.json",
                env.frontend(),
            )?;
            assert_cache_control(&redirect, CachePolicy::NoStoreMustRevalidate, &env.config());
            assert_eq!(redirect.headers()["access-control-allow-origin"], "*");

            Ok(())
        });
    }

    #[test_case("latest")]
    #[test_case("0.1")]
    #[test_case("0.1.0")]
    #[test_case("=0.1.0"; "exact_version")]
    fn failure(version: &str) {
        wrapper(|env| {
            env.fake_release()
                .name("foo")
                .version("0.1.0")
                .build_result_failed()
                .create()?;

            let response = env
                .frontend()
                .get(&format!("/crate/foo/{version}/status.json"))
                .send()?;
            assert_cache_control(&response, CachePolicy::NoStoreMustRevalidate, &env.config());
            assert_eq!(response.headers()["access-control-allow-origin"], "*");
            assert_eq!(response.status(), StatusCode::OK);
            let value: serde_json::Value = serde_json::from_str(&response.text()?)?;

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
    fn not_found(krate: &str, version: &str) {
        wrapper(|env| {
            env.fake_release().name("foo").version("0.1.1").create()?;

            let response = env
                .frontend()
                .get(&format!("/crate/{krate}/{version}/status.json"))
                .send()?;
            assert_cache_control(&response, CachePolicy::NoStoreMustRevalidate, &env.config());
            assert_eq!(response.headers()["access-control-allow-origin"], "*");
            assert_eq!(response.status(), StatusCode::NOT_FOUND);
            Ok(())
        });
    }
}
