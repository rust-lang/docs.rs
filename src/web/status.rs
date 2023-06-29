use super::cache::CachePolicy;
use crate::{
    db::Pool,
    utils::spawn_blocking,
    web::{error::AxumResult, match_version_axum},
};
use axum::{
    extract::{Extension, Path},
    http::header::ACCESS_CONTROL_ALLOW_ORIGIN,
    response::IntoResponse,
    Json,
};

pub(crate) async fn status_handler(
    Path((name, req_version)): Path<(String, String)>,
    Extension(pool): Extension<Pool>,
) -> AxumResult<impl IntoResponse> {
    let (_, id) = match_version_axum(&pool, &name, Some(&req_version))
        .await?
        .exact_name_only()?
        .exact_version_only()?;

    let rustdoc_status: bool = spawn_blocking({
        move || {
            Ok(pool
                .get()?
                .query_one(
                    "SELECT releases.rustdoc_status
                     FROM releases
                     WHERE releases.id = $1
                    ",
                    &[&id],
                )?
                .get("rustdoc_status"))
        }
    })
    .await?;

    Ok((
        Extension(CachePolicy::NoStoreMustRevalidate),
        [(ACCESS_CONTROL_ALLOW_ORIGIN, "*")],
        Json(serde_json::json!({ "doc_status": rustdoc_status })),
    ))
}

#[cfg(test)]
mod tests {
    use crate::{
        test::{assert_cache_control, wrapper},
        web::cache::CachePolicy,
    };
    use reqwest::StatusCode;

    #[test]
    fn success() {
        wrapper(|env| {
            env.fake_release().name("foo").version("0.1.0").create()?;

            let response = env.frontend().get("/crate/foo/0.1.0/status.json").send()?;
            assert_cache_control(&response, CachePolicy::NoStoreMustRevalidate, &env.config());
            assert_eq!(response.headers()["access-control-allow-origin"], "*");
            let value: serde_json::Value = serde_json::from_str(&response.text()?)?;

            assert_eq!(value, serde_json::json!({"doc_status": true}));

            Ok(())
        });
    }

    #[test]
    fn failure() {
        wrapper(|env| {
            env.fake_release()
                .name("foo")
                .version("0.1.0")
                .build_result_failed()
                .create()?;

            let response = env.frontend().get("/crate/foo/0.1.0/status.json").send()?;
            assert_cache_control(&response, CachePolicy::NoStoreMustRevalidate, &env.config());
            assert_eq!(response.headers()["access-control-allow-origin"], "*");
            let value: serde_json::Value = serde_json::from_str(&response.text()?)?;

            assert_eq!(value, serde_json::json!({"doc_status": false}));

            Ok(())
        });
    }

    #[test]
    fn crate_version_not_found() {
        wrapper(|env| {
            env.fake_release().name("foo").version("0.1.0").create()?;

            let response = env.frontend().get("/crate/foo/0.2.0/status.json").send()?;
            assert!(response
                .url()
                .as_str()
                .ends_with("/crate/foo/0.2.0/status.json"));
            assert_eq!(response.status(), StatusCode::NOT_FOUND);
            Ok(())
        });
    }

    #[test]
    fn invalid_semver() {
        wrapper(|env| {
            env.fake_release().name("foo").version("0.1.0").create()?;

            let response = env.frontend().get("/crate/foo/0,1,0/status.json").send()?;
            assert!(response
                .url()
                .as_str()
                .ends_with("/crate/foo/0,1,0/status.json"));
            assert_eq!(response.status(), StatusCode::NOT_FOUND);
            Ok(())
        });
    }

    /// We only support asking for the status of exact versions
    #[test]
    fn no_semver() {
        wrapper(|env| {
            env.fake_release().name("foo").version("0.1.0").create()?;

            let response = env.frontend().get("/crate/foo/latest/status.json").send()?;
            assert!(response
                .url()
                .as_str()
                .ends_with("/crate/foo/latest/status.json"));
            assert_eq!(response.status(), StatusCode::NOT_FOUND);

            let response = env.frontend().get("/crate/foo/0.1/status.json").send()?;
            assert!(response
                .url()
                .as_str()
                .ends_with("/crate/foo/0.1/status.json"));
            assert_eq!(response.status(), StatusCode::NOT_FOUND);

            Ok(())
        });
    }
}
