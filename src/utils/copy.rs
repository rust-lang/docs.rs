use std::ffi::OsStr;
use std::fs;
use std::io;
use std::path::Path;

use crate::error::Result;
use regex::Regex;

/// Copies documentation from a crate's target directory to destination.
///
/// Target directory must have doc directory.
///
/// This function is designed to avoid file duplications.
pub(crate) fn copy_doc_dir(source: impl AsRef<Path>, destination: impl AsRef<Path>) -> Result<()> {
    // Avoid copying common files
    let dup_regex = Regex::new(
        r"(\.lock|\.txt|\.woff|\.svg|\.css|main-.*\.css|main-.*\.js|normalize-.*\.js|rustdoc-.*\.css|storage-.*\.js|theme-.*\.js)$")
        .unwrap();

    copy_dir_all(source, destination, |filename| {
        dup_regex.is_match(filename.to_str().unwrap())
    })
    .map_err(Into::into)
}

pub(crate) fn copy_dir_all(
    src: impl AsRef<Path>,
    dst: impl AsRef<Path>,
    should_copy: impl Copy + Fn(&OsStr) -> bool,
) -> io::Result<()> {
    let dst = dst.as_ref();
    fs::create_dir_all(dst)?;
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let filename = entry.file_name();
        if entry.file_type()?.is_dir() {
            copy_dir_all(entry.path(), dst.join(filename), should_copy)?;
        } else if should_copy(&filename) {
            fs::copy(entry.path(), dst.join(filename))?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod test {
    use super::*;
    use std::fs;

    #[test]
    fn test_copy_doc_dir() {
        let source = tempfile::Builder::new()
            .prefix("cratesfyi-src")
            .tempdir()
            .unwrap();
        let destination = tempfile::Builder::new()
            .prefix("cratesfyi-dst")
            .tempdir()
            .unwrap();
        let doc = source.path().join("doc");
        fs::create_dir(&doc).unwrap();
        fs::create_dir(doc.join("inner")).unwrap();

        fs::write(doc.join("index.html"), "<html>spooky</html>").unwrap();
        fs::write(doc.join("index.txt"), "spooky").unwrap();
        fs::write(doc.join("inner").join("index.html"), "<html>spooky</html>").unwrap();
        fs::write(doc.join("inner").join("index.txt"), "spooky").unwrap();
        fs::write(doc.join("inner").join("important.svg"), "<svg></svg>").unwrap();

        // lets try to copy a src directory to tempdir
        copy_doc_dir(source.path().join("doc"), destination.path()).unwrap();
        assert!(destination.path().join("index.html").exists());
        assert!(!destination.path().join("index.txt").exists());
        assert!(destination.path().join("inner").join("index.html").exists());
        assert!(!destination.path().join("inner").join("index.txt").exists());
        assert!(!destination
            .path()
            .join("inner")
            .join("important.svg")
            .exists());
    }
}
