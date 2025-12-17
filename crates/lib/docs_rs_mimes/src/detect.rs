use mime::{self, Mime};
use std::{ffi::OsStr, path::Path};

pub fn detect_mime(file_path: impl AsRef<Path>) -> Mime {
    let mime = mime_guess::from_path(file_path.as_ref())
        .first()
        .unwrap_or(mime::TEXT_PLAIN);

    match mime.as_ref() {
        "text/plain" | "text/troff" | "text/x-markdown" | "text/x-rust" | "text/x-toml" => {
            match file_path.as_ref().extension().and_then(OsStr::to_str) {
                Some("md") => crate::TEXT_MARKDOWN.clone(),
                Some("rs") => crate::TEXT_RUST.clone(),
                Some("markdown") => crate::TEXT_MARKDOWN.clone(),
                Some("css") => mime::TEXT_CSS,
                Some("toml") => crate::TEXT_TOML.clone(),
                Some("js") => mime::TEXT_JAVASCRIPT,
                Some("json") => mime::APPLICATION_JSON,
                Some("gz") => crate::APPLICATION_GZIP.clone(),
                Some("zst") => crate::APPLICATION_ZSTD.clone(),
                _ => mime,
            }
        }
        "image/svg" => mime::IMAGE_SVG,

        _ => mime,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use test_case::test_case;

    // some standard mime types that mime-guess handles
    #[test_case("txt", &mime::TEXT_PLAIN)]
    #[test_case("html", &mime::TEXT_HTML)]
    // overrides of other mime types and defaults for
    // types mime-guess doesn't know about
    #[test_case("md", &crate::TEXT_MARKDOWN)]
    #[test_case("rs", &crate::TEXT_RUST)]
    #[test_case("markdown", &crate::TEXT_MARKDOWN)]
    #[test_case("css", &mime::TEXT_CSS)]
    #[test_case("toml", &crate::TEXT_TOML)]
    #[test_case("js", &mime::TEXT_JAVASCRIPT)]
    #[test_case("json", &mime::APPLICATION_JSON)]
    #[test_case("zst", &crate::APPLICATION_ZSTD)]
    #[test_case("gz", &crate::APPLICATION_GZIP)]
    fn test_detect_mime(ext: &str, expected: &Mime) {
        assert_eq!(&detect_mime(format!("something.{ext}")), expected);
    }

    #[test]
    fn test_mime_types() {
        check_mime(".gitignore", "text/plain");
        check_mime("hello.toml", "text/toml");
        check_mime("hello.css", "text/css");
        check_mime("hello.js", "text/javascript");
        check_mime("hello.html", "text/html");
        check_mime("hello.hello.md", "text/markdown");
        check_mime("hello.markdown", "text/markdown");
        check_mime("hello.json", "application/json");
        check_mime("hello.txt", "text/plain");
        check_mime("file.rs", "text/rust");
        check_mime("important.svg", "image/svg+xml");
    }

    fn check_mime(path: &str, expected_mime: &str) {
        let detected_mime = detect_mime(Path::new(&path));
        assert_eq!(detected_mime, expected_mime);
    }
}
