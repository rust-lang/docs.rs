use mime::Mime;
use once_cell::sync::Lazy;

macro_rules! mime {
    ($id:ident, $mime:expr) => {
        pub(crate) static $id: Lazy<Mime> = Lazy::new(|| $mime.parse().unwrap());
    };
}

mime!(APPLICATION_ZIP, "application/zip");
mime!(APPLICATION_ZSTD, "application/zstd");
mime!(APPLICATION_GZIP, "application/gzip");
mime!(TEXT_MARKDOWN, "text/markdown");
mime!(TEXT_RUST, "text/rust");
mime!(TEXT_TOML, "text/toml");
