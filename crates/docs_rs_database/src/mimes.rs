use mime::Mime;
use std::{ffi::OsStr, path::Path, sync::LazyLock};

macro_rules! mime {
    ($id:ident, $mime:expr) => {
        pub static $id: LazyLock<Mime> = LazyLock::new(|| $mime.parse().unwrap());
    };
}

mime!(APPLICATION_ZIP, "application/zip");
mime!(APPLICATION_ZSTD, "application/zstd");
mime!(APPLICATION_GZIP, "application/gzip");
mime!(
    APPLICATION_OPENSEARCH_XML,
    "application/opensearchdescription+xml"
);
mime!(APPLICATION_XML, "application/xml");
mime!(TEXT_MARKDOWN, "text/markdown");
mime!(TEXT_RUST, "text/rust");
mime!(TEXT_TOML, "text/toml");

pub fn detect_mime(file_path: impl AsRef<Path>) -> Mime {
    let mime = mime_guess::from_path(file_path.as_ref())
        .first()
        .unwrap_or(mime::TEXT_PLAIN);

    match mime.as_ref() {
        "text/plain" | "text/troff" | "text/x-markdown" | "text/x-rust" | "text/x-toml" => {
            match file_path.as_ref().extension().and_then(OsStr::to_str) {
                Some("md") => TEXT_MARKDOWN.clone(),
                Some("rs") => TEXT_RUST.clone(),
                Some("markdown") => TEXT_MARKDOWN.clone(),
                Some("css") => mime::TEXT_CSS,
                Some("toml") => TEXT_TOML.clone(),
                Some("js") => mime::TEXT_JAVASCRIPT,
                Some("json") => mime::APPLICATION_JSON,
                Some("gz") => APPLICATION_GZIP.clone(),
                Some("zst") => APPLICATION_ZSTD.clone(),
                _ => mime,
            }
        }
        "image/svg" => mime::IMAGE_SVG,

        _ => mime,
    }
}
