//! The docs.rs API, things are versioned for future compatibility

/// Unwraps an `Option`, returning an api error with the given message on a `None`
macro_rules! api_error {
    ($expr:expr, $msg:expr $(,)?) => {
        if let Some(val) = $expr {
            val
        } else {
            return ApiErrorV1::new($msg).into_response();
        }
    };
}

mod badges;

pub use badges::badge_handler_v1;

use super::Pool;
use iron::{headers::ContentType, status, IronResult, Response};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct ApiErrorV1 {
    message: String,
    documentation_url: String,
}

impl ApiErrorV1 {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            documentation_url: "https://github.com/rust-lang/docs.rs".to_string(),
        }
    }

    pub fn into_response(self) -> IronResult<Response> {
        let mut response =
            Response::with((status::NotFound, serde_json::to_string(&self).unwrap()));
        response
            .headers
            .set(ContentType("application/json".parse().unwrap()));

        Ok(response)
    }
}
