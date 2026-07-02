use crate::{FileEntry, Manifest};
use anyhow::Result;
use docs_rs_types::{KrateName, Version};
use reqwest::header::{HeaderMap, RANGE};
use std::io::{self, Write as _};
use tokio::sync::Mutex;
use zip::{CompressionMethod, ZipWriter, write::SimpleFileOptions};

pub fn create_test_source_archive<I, N, B>(files: I) -> Result<(Manifest, Vec<u8>)>
where
    I: IntoIterator<Item = (N, B)>,
    N: ToString,
    B: AsRef<[u8]>,
{
    let options = SimpleFileOptions::default()
        .compression_method(CompressionMethod::Deflated)
        .compression_level(Some(9));

    let buf = Vec::new();
    let mut zip = ZipWriter::new(io::Cursor::new(buf));

    for (filename, content) in files {
        zip.start_file(filename, options)?;
        zip.write_all(content.as_ref())?;
    }

    let mut archive = zip.finish_into_readable()?;

    let mut files = Vec::with_capacity(archive.len());
    for i in 0..archive.len() {
        // `_raw` because we only read each entry's metadata, never its bytes,
        // so there is no need to set up a decompressor.
        let entry = archive.by_index_raw(i)?;

        let path = entry.name().to_string();
        let data_offset = entry.data_start().expect("missing data start");

        debug_assert!(matches!(entry.compression(), CompressionMethod::Deflated));

        files.push(FileEntry {
            data_offset,
            compressed_size: entry.compressed_size(),
            uncompressed_size: entry.size(),
            compression: "deflate".into(),
            sha256: "dummy".into(),
            path,
        });
    }

    let bytes = archive.into_inner().into_inner();

    Ok((Manifest { files }, bytes))
}

pub struct TestStaticCratesIo {
    inner: Mutex<TestStaticCratesIoInner>,
}

struct TestStaticCratesIoInner {
    mocks: Vec<mockito::Mock>,
    server: mockito::ServerGuard,
}

impl TestStaticCratesIo {
    pub async fn new() -> Result<Self> {
        Ok(Self {
            inner: Mutex::new(TestStaticCratesIoInner {
                mocks: Vec::new(),
                server: mockito::Server::new_async().await,
            }),
        })
    }
    pub async fn add(
        &self,
        name: &KrateName,
        version: &Version,
        manifest: Manifest,
        zip: Vec<u8>,
    ) -> Result<()> {
        let mut inner = self.inner.lock().await;

        let mock_json = inner
            .server
            .mock("GET", &*format!("/crates/{name}/{name}-{version}.zip.json"))
            .with_body(serde_json::to_string(&manifest).unwrap())
            .create_async()
            .await;
        inner.mocks.push(mock_json);

        let mock_zip = inner
            .server
            .mock("GET", &*format!("/crates/{name}/{name}-{version}.zip"))
            .with_body_from_request(move |request| {
                let Some((lhs, rhs)) = request
                    .headers()
                    .get(RANGE)
                    .and_then(|h| h.to_str().ok())
                    .and_then(|s| s.strip_prefix("bytes="))
                    .and_then(|r| r.split_once("-"))
                    .and_then(|(lhs, rhs)| {
                        let lhs: usize = lhs.parse().ok()?;
                        let rhs: usize = rhs.parse().ok()?;
                        Some((lhs, rhs))
                    })
                else {
                    return zip.clone();
                };

                zip.get(lhs..=rhs).unwrap().to_vec()
            })
            .create_async()
            .await;
        inner.mocks.push(mock_zip);

        Ok(())
    }

    pub async fn url(&self) -> String {
        let inner = self.inner.lock().await;
        inner.server.url()
    }
}
