use anyhow::Result;
use percent_encoding::{AsciiSet, CONTROLS, utf8_percent_encode};
use std::borrow::Cow;

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

#[cfg(test)]
mod test {
    use super::*;
    use test_case::test_case;

    #[test_case("/something/", "/something/")] // already valid path
    #[test_case("/something>", "/something%3E")] // something to encode
    #[test_case("/something%3E", "/something%3E")] // re-running doesn't change anything
    fn test_encode_url_path(input: &str, expected: &str) {
        assert_eq!(encode_url_path(input), expected);
    }
}
