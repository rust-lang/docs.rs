
use std::path::Path;
use std::fs;
use error::Result;

use regex::Regex;

/// Copies documentation from a crate's target directory to destination.
///
/// Target directory must have doc directory.
///
/// This function is designed to avoid file duplications.
pub fn copy_doc_dir<P: AsRef<Path>, Q: AsRef<Path>>(source: P, destination: Q) -> Result<()> {
    let destination = destination.as_ref();

    // Make sure destination directory exists
    if !destination.exists() {
        fs::create_dir_all(destination)?;
    }

    // Avoid copying common files
    let dup_regex = Regex::new(
        r"(\.lock|\.txt|\.woff|\.svg|\.css|main-.*\.css|main-.*\.js|normalize-.*\.js|rustdoc-.*\.css|storage-.*\.js|theme-.*\.js)$")
        .unwrap();

    for file in source.as_ref().read_dir()? {

        let file = file?;
        let destination_full_path = destination.join(file.file_name());

        let metadata = file.metadata()?;

        if metadata.is_dir() {
            fs::create_dir_all(&destination_full_path)?;
            copy_doc_dir(file.path(), destination_full_path)?
        } else if dup_regex.is_match(&file.file_name().into_string().unwrap()[..]) {
            continue;
        } else {
            fs::copy(&file.path(), &destination_full_path)?;
        }

    }
    Ok(())
}



#[cfg(test)]
mod test {
    extern crate env_logger;
    use std::fs;
    use super::*;

    #[test]
    fn test_copy_doc_dir() {
        let source = tempdir::TempDir::new("cratesfyi-src").unwrap();
        let destination = tempdir::TempDir::new("cratesfyi-dst").unwrap();
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
        assert!(!destination.path().join("inner").join("important.svg").exists());
    }
}
