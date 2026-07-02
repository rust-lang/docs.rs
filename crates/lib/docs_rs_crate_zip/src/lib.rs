//! Library to read the crates.io source archives (manifest & zip), and
//! fetch single files from the remote archives.
//!
//! Archives are created here:
//! https://github.com/rust-lang/crates.io/blob/5274087feb193ee490e9a6bbdf2e18e74e9ddaeb/crates/crates_io_crate_zip/src/lib.rs
//! Also we copied the manifest structs from there.

#[cfg(any(test, feature = "testing"))]
pub mod test_env;

use anyhow::{Result, bail};
use async_compression::tokio::bufread::DeflateDecoder;
use docs_rs_utils::APP_USER_AGENT;
use futures_util::TryStreamExt as _;
use reqwest::{
    IntoUrl, StatusCode, Url,
    header::{HeaderValue, RANGE, USER_AGENT},
};
use serde::{Deserialize, Serialize};
use tokio::io::{self, AsyncWrite, AsyncWriteExt as _};
use tokio_util::io::StreamReader;

/// archive manifest serde structs, copied from
/// https://github.com/rust-lang/crates.io/blob/5274087feb193ee490e9a6bbdf2e18e74e9ddaeb/crates/crates_io_crate_zip/src/lib.rs
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Manifest {
    /// One entry per file in the zip, sorted alphabetically by path.
    pub files: Vec<FileEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct FileEntry {
    /// Realtive path (without the leading `{name}-{version}/` component of
    /// the tarball).
    pub path: String,
    /// Byte offset in the zip where this entry's compressed payload begins.
    pub data_offset: u64,
    /// Length of the compressed contents in bytes.
    pub compressed_size: u64,
    /// Length of the uncompressed contents in bytes.
    pub uncompressed_size: u64,
    /// How the payload is compressed: `"deflate"` or `"store"`.
    pub compression: String,
    /// Lowercase hex sha256 of the uncompressed contents.
    pub sha256: String,
}

pub struct SourceArchive {
    manifest: Manifest,
    zip_url: Url,
    client: reqwest::Client,
}

impl SourceArchive {
    pub async fn load(name: impl AsRef<str>, version: impl AsRef<str>) -> Result<Option<Self>> {
        Self::load_from("https://static.crates.io/", name, version).await
    }

    pub async fn load_from(
        base_url: impl IntoUrl,
        name: impl AsRef<str>,
        version: impl AsRef<str>,
    ) -> Result<Option<Self>> {
        let mut base_url = base_url.into_url()?;
        base_url.set_path("crates/");

        let index_url = base_url.join(&format!(
            "{0}/{0}-{1}.zip.json",
            name.as_ref(),
            version.as_ref()
        ))?;

        let headers = vec![(USER_AGENT, HeaderValue::from_static(APP_USER_AGENT))]
            .into_iter()
            .collect();

        let client = reqwest::Client::builder()
            .default_headers(headers)
            .build()?;

        let response = client.get(index_url.clone()).send().await?;
        if matches!(
            response.status(),
            StatusCode::NOT_FOUND | StatusCode::FORBIDDEN
        ) {
            return Ok(None);
        }
        let response = response.error_for_status()?;

        Ok(Some(Self {
            manifest: response.json().await?,
            zip_url: base_url.join(&format!("{0}/{0}-{1}.zip", name.as_ref(), version.as_ref()))?,
            client,
        }))
    }

    pub fn entries(&self) -> impl Iterator<Item = &FileEntry> {
        self.manifest.files.iter()
    }

    pub fn by_name(&self, path: impl AsRef<str>) -> Option<&FileEntry> {
        let path = path.as_ref();
        self.manifest.files.iter().find(|e| e.path == path)
    }

    pub async fn fetch<W>(&self, entry: &FileEntry, writer: &mut W) -> Result<()>
    where
        W: AsyncWrite + Unpin,
    {
        let range_start = entry.data_offset;
        let range_end = entry.data_offset + entry.compressed_size - 1;

        let response = self
            .client
            .get(self.zip_url.clone())
            .header(RANGE, format!("bytes={range_start}-{range_end}",))
            .send()
            .await?
            .error_for_status()?;

        let stream = response.bytes_stream().map_err(std::io::Error::other);
        let mut reader = StreamReader::new(stream);

        match entry.compression.as_str() {
            "deflate" => {
                let mut decoder = DeflateDecoder::new(reader);
                io::copy(&mut decoder, writer).await?;
            }
            "store" => {
                io::copy(&mut reader, writer).await?;
            }
            compression => bail!("unsupported zip compression: {}", compression),
        }

        writer.flush().await?;

        Ok(())
    }

    pub async fn fetch_bytes(&self, entry: &FileEntry) -> Result<Vec<u8>> {
        let mut buf = Vec::new();
        self.fetch(entry, &mut buf).await?;
        Ok(buf)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_env::{TestEnv, create_test_archive};

    #[tokio::test]
    async fn test_fetch() -> anyhow::Result<()> {
        let (manifest, zip) =
            create_test_archive([("src/main.rs", "src/main.rs"), ("Cargo.toml", "Cargo.toml")])?;

        let test_env = TestEnv::new("krate", "0.1.0", manifest, zip).await?;

        let source_archive = SourceArchive::load_from(test_env.url(), "krate", "0.1.0")
            .await?
            .expect("not found");

        {
            let info = source_archive.by_name("src/main.rs").expect("should exist");
            assert_eq!(source_archive.fetch_bytes(&info).await?, b"src/main.rs");
        }

        {
            let info = source_archive.by_name("Cargo.toml").expect("should exist");
            assert_eq!(source_archive.fetch_bytes(&info).await?, b"Cargo.toml");
        }

        Ok(())
    }
}
