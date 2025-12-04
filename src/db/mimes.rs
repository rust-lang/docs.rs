use mime::Mime;
use std::sync::LazyLock;

macro_rules! mime {
    ($id:ident, $mime:expr) => {
        pub(crate) static $id: LazyLock<Mime> = LazyLock::new(|| $mime.parse().unwrap());
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
