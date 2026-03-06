use crate::common::download_to_temp_file;
use anyhow::{Result, bail};
use async_tar::Archive;
use docs_rs_storage::compression::wrap_reader_for_decompression;
use docs_rs_types::{CompressionAlgorithm, KrateName, Version};
use docs_rs_utils::spawn_blocking;
use std::path::{Path, PathBuf};
use tokio::io;
use tracing::debug;

#[derive(Debug)]
pub(crate) struct SourceDir {
    _temp_dir: tempfile::TempDir,
    pub(crate) source_path: PathBuf,
}

impl AsRef<Path> for SourceDir {
    fn as_ref(&self) -> &Path {
        &self.source_path
    }
}

pub(crate) async fn download_and_extract_source(
    name: &KrateName,
    version: &Version,
) -> Result<SourceDir> {
    debug!("downloading source");
    let crate_archive = download_to_temp_file(format!(
        "https://static.crates.io/crates/{name}/{name}-{version}.crate"
    ))
    .await?;

    let temp_dir = spawn_blocking(|| Ok(tempfile::tempdir()?)).await?;

    debug!("unpacking source archive");
    {
        let mut file = io::BufReader::new(crate_archive);
        let mut decompressed = wrap_reader_for_decompression(&mut file, CompressionAlgorithm::Gzip);
        let archive = Archive::new(&mut decompressed);
        archive.unpack(&temp_dir).await?;
    }

    let source_path = temp_dir.path().join(format!("{name}-{version}"));
    if !source_path.is_dir() {
        bail!(
            "broken crate archive, missing source directory {:?}",
            source_path
        );
    };

    Ok(SourceDir {
        source_path,
        _temp_dir: temp_dir,
    })
}
