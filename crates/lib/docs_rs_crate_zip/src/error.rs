use reqwest::Url;

#[derive(thiserror::Error, Debug)]
pub enum Error {
    #[error("manifest not found: {0}")]
    ManifestNotFound(Url),

    #[error("source archive not found: {0}, range={1}-{2}")]
    ArchiveNotFound(Url, u64, u64),

    #[error("url-parsing error")]
    UrlParse,

    #[error("i/o error: {0}")]
    Io(#[from] std::io::Error),

    #[error("reqwest error")]
    Request(#[from] reqwest::Error),

    #[error("unsupported zip compression method: {0}")]
    UnsupportedZipCompression(String),
}
