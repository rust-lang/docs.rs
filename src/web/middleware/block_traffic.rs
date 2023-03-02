//! Middleware that blocks requests if a header matches the given list
//!
//! To use, set the `BLOCKED_TRAFFIC` environment variable to a comma-separated list of pairs
//! containing a header name, an equals sign, and the name of another environment variable that
//! contains the values of that header that should be blocked. For example, set `BLOCKED_TRAFFIC`
//! to `User-Agent=BLOCKED_UAS,X-Real-Ip=BLOCKED_IPS`, `BLOCKED_UAS` to `curl/7.54.0,cargo 1.36.0
//! (c4fcfb725 2019-05-15)`, and `BLOCKED_IPS` to `192.168.0.1,127.0.0.1` to block requests from
//! the versions of curl or Cargo specified or from either of the IPs (values are nonsensical
//! examples). Values of the headers must match exactly.

use crate::Config;
use axum::{extract::Extension, middleware::Next, response::IntoResponse};
use http::StatusCode;
use std::sync::Arc;

pub async fn block_traffic<B>(
    Extension(config): Extension<Arc<Config>>,
    req: http::Request<B>,
    next: Next<B>,
) -> axum::response::Response {
    let blocked_traffic = &config.blocked_traffic;

    for (header_name, blocked_values) in blocked_traffic {
        let has_blocked_value = req
            .headers()
            .get_all(header_name)
            .iter()
            .any(|value| blocked_values.iter().any(|v| v == value));
        if has_blocked_value {
            tracing::warn!("blocked due to contents of header {header_name}");

            let body = "We are unable to process your request at this time. \
                 This usually means that you are in violation of our crawler \
                 policy (https://docs.rs/about#crawlers). \
                 Please open an issue at https://github.com/rust-lang/docs.rs if 
                 for help.";

            return (StatusCode::FORBIDDEN, body).into_response();
        }
    }

    next.run(req).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test::wrapper;
    use reqwest::header::USER_AGENT;

    #[test]
    fn blocked_traffic_doesnt_panic_if_checked_header_is_not_present() {
        wrapper(|env| {
            env.override_config(|config| {
                config.blocked_traffic = vec![("Never-Given".into(), vec!["1".into()])];
            });

            let web = env.frontend();

            let response = web.get("/").header(USER_AGENT, "").send()?;
            assert_eq!(response.status(), StatusCode::OK, "{}", response.text()?);
            Ok(())
        })
    }

    #[test]
    fn block_traffic_via_arbitrary_header_and_value() {
        wrapper(|env| {
            env.override_config(|config| {
                config.blocked_traffic = vec![("User-Agent".into(), vec!["1".into(), "2".into()])];
            });

            let web = env.frontend();

            let response = web.get("/").header(USER_AGENT, "1").send()?;
            assert_eq!(
                response.status(),
                StatusCode::FORBIDDEN,
                "{}",
                response.text()?
            );

            // A request with a header value we don't want to block is allowed, even though there might
            // be a substring match
            let response = web
                .get("/")
                .header(USER_AGENT, "1value-must-match-exactly-this-is-allowed")
                .send()?;
            assert_eq!(response.status(), StatusCode::OK, "{}", response.text()?);
            Ok(())
        })
    }
}
