use crc32fast::Hasher;
use std::{
    fs,
    io::{self, Read, Seek as _},
    path::Path,
};

pub fn crc32_for_path(path: impl AsRef<Path>) -> Result<[u8; 4], io::Error> {
    let path = path.as_ref();

    let mut file = fs::File::open(path)?;
    crc32_for_reader(&mut file, u64::MAX)
}

pub fn crc32_for_path_range(
    path: impl AsRef<Path>,
    offset: u64,
    length: u64,
) -> Result<[u8; 4], io::Error> {
    let path = path.as_ref();

    let mut file = fs::File::open(path)?;
    file.seek(io::SeekFrom::Start(offset))?;
    crc32_for_reader(&mut file, length)
}

fn crc32_for_reader(mut reader: impl Read, length: u64) -> Result<[u8; 4], io::Error> {
    let mut hasher = Hasher::new();
    let mut buffer = [0; 256 * 1024];
    let mut remaining = length;

    while remaining > 0 {
        let max_read = buffer.len().min(remaining as usize);
        let read = reader.read(&mut buffer[..max_read])?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
        remaining -= read as u64;
    }

    Ok(hasher.finalize().to_be_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;
    use anyhow::Result;

    fn write_fixture(content: &[u8]) -> Result<tempfile::NamedTempFile> {
        let mut file = tempfile::NamedTempFile::new()?;
        io::Write::write_all(&mut file, content)?;
        Ok(file)
    }

    #[test]
    fn crc32_for_path_matches_known_value() -> Result<()> {
        let file = write_fixture(b"123456789")?;

        assert_eq!(crc32_for_path(file.path())?, 0xcbf4_3926u32.to_be_bytes());

        Ok(())
    }

    #[test]
    fn crc32_for_path_range_matches_subset() -> Result<()> {
        let file = write_fixture(b"abcdefghij")?;

        assert_eq!(
            crc32_for_path_range(file.path(), 2, 4)?,
            crc32_for_reader("cdef".as_bytes(), u64::MAX)?
        );

        Ok(())
    }

    #[test]
    fn crc32_for_path_range_with_zero_length_hashes_empty_input() -> Result<()> {
        let file = write_fixture(b"abcdefghij")?;

        assert_eq!(
            crc32_for_path_range(file.path(), 2, 0)?,
            crc32_for_reader(io::empty(), u64::MAX)?
        );

        Ok(())
    }

    #[test]
    fn crc32_for_path_range_stops_at_eof() -> Result<()> {
        let file = write_fixture(b"abcdefghij")?;

        assert_eq!(
            crc32_for_path_range(file.path(), 8, 100)?,
            crc32_for_reader("ij".as_bytes(), u64::MAX)?
        );

        Ok(())
    }

    #[test]
    fn crc32_for_path_errors_for_missing_file() {
        let err =
            crc32_for_path("/definitely/not/a/docsrs/file").expect_err("missing file should fail");

        assert_eq!(err.kind(), io::ErrorKind::NotFound);
    }
}
