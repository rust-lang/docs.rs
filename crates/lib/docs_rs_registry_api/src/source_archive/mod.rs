pub mod manifest;

use crate::source_archive::manifest::{FileEntry, Manifest};
use anyhow::{Result, bail};
use async_compression::tokio::bufread::DeflateDecoder;
use futures_util::TryStreamExt as _;
use reqwest::{IntoUrl, StatusCode, Url, header::RANGE};
use tokio::io::{self, AsyncWrite, AsyncWriteExt as _};
use tokio_util::io::StreamReader;

pub struct SourceArchive {
    manifest: Manifest,
    zip_url: Url,
    client: reqwest::Client,
}

impl SourceArchive {
    pub(crate) async fn load(
        client: reqwest::Client,
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
    use crate::testing::static_test_env::{TestStaticCratesIo, create_test_source_archive};

    #[tokio::test]
    async fn test_fetch() -> anyhow::Result<()> {
        let (manifest, zip) = create_test_source_archive([
            ("src/main.rs", "src/main.rs"),
            ("Cargo.toml", "Cargo.toml"),
        ])?;

        let mut test_env = TestStaticCratesIo::new().await?;
        test_env.add("krate", "0.1.0", manifest, zip).await?;

        let source_archive =
            SourceArchive::load(test_env.client().clone(), test_env.url(), "krate", "0.1.0")
                .await?
                .expect("not found");

        {
            let info = source_archive.by_name("src/main.rs").expect("should exist");
            assert_eq!(source_archive.fetch_bytes(info).await?, b"src/main.rs");
        }

        {
            let info = source_archive.by_name("Cargo.toml").expect("should exist");
            assert_eq!(source_archive.fetch_bytes(info).await?, b"Cargo.toml");
        }

        Ok(())
    }
}
