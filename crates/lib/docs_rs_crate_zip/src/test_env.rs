use crate::{FileEntry, Manifest};
use anyhow::Result;
use reqwest::header::RANGE;
use std::io::{self, Write as _};
use zip::{CompressionMethod, ZipWriter, write::SimpleFileOptions};

pub fn create_test_archive<I, N, B>(files: I) -> Result<(Manifest, Vec<u8>)>
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

pub struct TestEnv {
    mocks: Vec<mockito::Mock>,
    server: mockito::ServerGuard,
}

impl TestEnv {
    pub async fn new() -> Result<Self> {
        Ok(Self {
            mocks: Vec::new(),
            server: mockito::Server::new_async().await,
        })
    }
    pub async fn add(
        &mut self,
        name: impl AsRef<str>,
        version: impl AsRef<str>,
        manifest: Manifest,
        zip: Vec<u8>,
    ) -> Result<()> {
        let name = name.as_ref();
        let version = version.as_ref();

        self.mocks.push(
            self.server
                .mock("GET", &*format!("/crates/{name}/{name}-{version}.zip.json"))
                .with_body(serde_json::to_string(&manifest).unwrap())
                .create_async()
                .await,
        );

        self.mocks.push(
            self.server
                .mock("GET", &*format!("/crates/{name}/{name}-{version}.zip"))
                .with_body_from_request(move |request| {
                    let range = request
                        .headers()
                        .get(RANGE)
                        .expect("range header must exists");
                    let range = range.to_str().unwrap();

                    let bytes = range.strip_prefix("bytes=").unwrap();

                    let (lhs, rhs) = bytes.split_once("-").unwrap();

                    let lhs: usize = lhs.parse().unwrap();
                    let rhs: usize = rhs.parse().unwrap();

                    zip.get(lhs..=rhs).unwrap().to_vec()
                })
                .create_async()
                .await,
        );

        Ok(())
    }

    pub fn url(&self) -> String {
        self.server.url()
    }
}
