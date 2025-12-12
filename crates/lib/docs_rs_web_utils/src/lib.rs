use std::borrow::Cow;

use anyhow::Result;
use percent_encoding::{AsciiSet, CONTROLS, utf8_percent_encode};

pub mod escaped_uri;

// from https://github.com/servo/rust-url/blob/master/url/src/parser.rs
// and https://github.com/tokio-rs/axum/blob/main/axum-extra/src/lib.rs
const FRAGMENT: &AsciiSet = &CONTROLS.add(b' ').add(b'"').add(b'<').add(b'>').add(b'`');
const PATH: &AsciiSet = &FRAGMENT.add(b'#').add(b'?').add(b'{').add(b'}');

pub fn encode_url_path(path: &str) -> String {
    utf8_percent_encode(path, PATH).to_string()
}

pub fn url_decode<'a>(input: &'a str) -> Result<Cow<'a, str>> {
    Ok(percent_encoding::percent_decode(input.as_bytes()).decode_utf8()?)
}
