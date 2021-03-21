use crate::error::Result;
use std::fs;
use std::path::{Path, PathBuf};

/// Copies documentation from a crate's target directory to destination.
///
/// Target directory must have doc directory.
///
/// This does not copy any files with the same name as `shared_files`.
pub fn copy_doc_dir<P: AsRef<Path>, Q: AsRef<Path>>(
    source: P,
    destination: Q,
    shared_files: &[PathBuf],
) -> Result<()> {
    let destination = destination.as_ref();

    // Make sure destination directory exists
    if !destination.exists() {
        fs::create_dir_all(destination)?;
    }

    for file in source.as_ref().read_dir()? {
        let file = file?;
        let filename = file.file_name();
        let destination_full_path = destination.join(&filename);

        let metadata = file.metadata()?;

        if metadata.is_dir() {
            copy_doc_dir(file.path(), destination_full_path, shared_files)?;
            continue;
        }

        if shared_files.contains(&PathBuf::from(filename)) {
            continue;
        } else {
            fs::copy(&file.path(), &destination_full_path)?;
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
        let ignored_files = ["index.txt".into(), "important.svg".into()];
        copy_doc_dir(
            source.path().join("doc"),
            destination.path(),
            &ignored_files,
        )
        .unwrap();
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
