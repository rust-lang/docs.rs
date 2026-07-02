pub mod manifest;

use crate::source_archive::manifest::{FileEntry, Manifest};
use anyhow::{Result, bail};
use async_compression::tokio::bufread::DeflateDecoder;
use futures_util::TryStreamExt as _;
use reqwest::{
    StatusCode, Url,
    header::{HeaderMap, HeaderName, RANGE},
};
use tokio::io::{self, AsyncWrite, AsyncWriteExt as _};
use tokio_util::io::StreamReader;
use tracing::{debug, field, instrument};

pub static X_CACHE: HeaderName = HeaderName::from_static("x-cache");

fn is_cache_hit(hm: &HeaderMap) -> bool {
    hm.get(&X_CACHE)
        .and_then(|hv| hv.to_str().ok())
        .map(|hv| hv.contains("HIT"))
        .unwrap_or(false)
}

pub struct SourceArchive {
    manifest: Manifest,
    zip_url: Url,
    client: reqwest::Client,
}

impl SourceArchive {
    #[instrument(skip_all, fields( %base_url, %name, %version, cache_hit=field::Empty))]
    pub(crate) async fn load(
        client: reqwest::Client,
        mut base_url: Url,
        name: &str,
        version: &str,
    ) -> Result<Option<Self>> {
        base_url.set_path("crates/");

        let index_url = base_url.join(&format!("{0}/{0}-{1}.zip.json", name, version))?;

        debug!(%index_url, "fetching source archive manifest");
        let response = client.get(index_url.clone()).send().await?;
        if matches!(
            response.status(),
            StatusCode::NOT_FOUND | StatusCode::FORBIDDEN
        ) {
            return Ok(None);
        }
        let response = response.error_for_status()?;

        tracing::Span::current().record("cache_hit", is_cache_hit(response.headers()));

        Ok(Some(Self {
            manifest: response.json().await?,
            zip_url: base_url.join(&format!("{0}/{0}-{1}.zip", name, version))?,
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

    #[instrument(skip_all, fields(zip_url=%self.zip_url, path=%entry.path, cache_hit=field::Empty))]
    pub async fn fetch<W>(&self, entry: &FileEntry, writer: &mut W) -> Result<()>
    where
        W: AsyncWrite + Unpin,
    {
        let range_start = entry.data_offset;
        let range_end = entry.data_offset + entry.compressed_size - 1;

        debug!(range_start, range_end, "fetching file from source archive");
        let response = self
            .client
            .get(self.zip_url.clone())
            .header(RANGE, format!("bytes={range_start}-{range_end}",))
            .send()
            .await?
            .error_for_status()?;

        tracing::Span::current().record("cache_hit", is_cache_hit(response.headers()));

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
    use docs_rs_types::testing::{KRATE, V0_1};

    fn client() -> reqwest::Client {
        reqwest::Client::builder().build().unwrap()
    }

    #[tokio::test]
    async fn test_fetch() -> anyhow::Result<()> {
        let (manifest, zip) = create_test_source_archive([
            ("src/main.rs", "src/main.rs"),
            ("Cargo.toml", "Cargo.toml"),
        ])?;

        let test_env = TestStaticCratesIo::new().await?;
        test_env.add(&KRATE, &V0_1, manifest, zip).await?;

        let source_archive = SourceArchive::load(client(), test_env.url().await, "krate", "0.1.0")
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
