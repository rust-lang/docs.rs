//! Read crate package archives.

use anyhow::{Context as _, Result, bail};
use flate2::read::GzDecoder;
use std::{
    fs,
    io::Read,
    path::{Path, PathBuf},
};

/// A crate archive extracted into a temporary source directory.
///
/// Keeping this value alive keeps the source directory alive.
#[derive(Debug)]
pub struct SourceDir {
    _temporary: tempfile::TempDir,
    source_dir: PathBuf,
}

impl SourceDir {
    /// Return the root directory of the unpacked crate source.
    pub fn path(&self) -> &Path {
        &self.source_dir
    }
}

impl AsRef<Path> for SourceDir {
    fn as_ref(&self) -> &Path {
        self.path()
    }
}

/// Gzip-decompress and unpack a `.crate` archive.
///
/// The archive must contain exactly one top-level directory, which is returned as [`SourceDir`].
pub fn unpack_crate_archive(archive: impl Read) -> Result<SourceDir> {
    let temporary = tempfile::tempdir().context("creating temporary source directory")?;
    tar::Archive::new(GzDecoder::new(archive))
        .unpack(temporary.path())
        .context("extracting crate archive")?;

    let entries = fs::read_dir(temporary.path())
        .context("reading extracted crate archive")?
        .collect::<std::result::Result<Vec<_>, _>>()?;

    let source_dir = match entries.as_slice() {
        [entry] if entry.file_type()?.is_dir() => entry.path(),
        _ => bail!(
            "expected the crate archive to contain one root directory, found {} entries",
            entries.len()
        ),
    };

    Ok(SourceDir {
        _temporary: temporary,
        source_dir,
    })
}

#[cfg(any(test, feature = "testing"))]
/// Test utilities for creating crate package archives.
pub mod testing {
    use super::*;
    use docs_rs_types::{KrateName, Version};
    use flate2::write::GzEncoder;

    /// Create a gzip-compressed crate archive from a crate source root.
    ///
    /// The archive contains `root` beneath a single `<name>-<version>` top-level directory.
    pub fn create_source_tarball(
        name: &KrateName,
        version: &Version,
        root: impl AsRef<Path>,
    ) -> Result<Vec<u8>> {
        let root = root.as_ref();
        let encoder = GzEncoder::new(Vec::new(), flate2::Compression::default());
        let mut archive = tar::Builder::new(encoder);
        archive.append_dir_all(format!("{name}-{version}"), root)?;
        Ok(archive.into_inner()?.finish()?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn unpacks_the_single_source_root() -> Result<()> {
        let root = tempfile::tempdir()?;
        fs::write(
            root.path().join("Cargo.toml"),
            "[package]\nname = \"krate\"\n",
        )?;
        let name = "krate".parse()?;
        let version = "1.0.0".parse()?;
        let archive = testing::create_source_tarball(&name, &version, &root)?;

        let source = unpack_crate_archive(Cursor::new(archive))?;
        assert_eq!(
            fs::read_to_string(source.path().join("Cargo.toml"))?,
            "[package]\nname = \"krate\"\n"
        );

        Ok(())
    }
}
