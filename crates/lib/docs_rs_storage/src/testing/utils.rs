use std::io;
use zip;

/// try decompressing the zip & read the content
pub fn check_archive_consistency(compressed_body: &[u8]) -> anyhow::Result<()> {
    let mut zip = zip::ZipArchive::new(io::Cursor::new(compressed_body))?;
    for i in 0..zip.len() {
        let mut file = zip.by_index(i)?;

        let mut buf = Vec::new();
        io::copy(&mut file, &mut buf)?;
    }

    Ok(())
}
