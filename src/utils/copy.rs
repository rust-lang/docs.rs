
// FIXME: There is so many PathBuf's in this module
//        Conver them to Path

use std::path::{Path, PathBuf};
use std::fs;
use error::Result;

use regex::Regex;

/// Copies documentation from a crate's target directory to destination.
///
/// Target directory must have doc directory.
///
/// This function is designed to avoid file duplications.
pub fn copy_doc_dir<P: AsRef<Path>>(target: P, destination: P) -> Result<()> {
    let source = PathBuf::from(target.as_ref()).join("doc");
    let destination = destination.as_ref().to_path_buf();

    // Make sure destination directory exists
    if !destination.exists() {
        fs::create_dir_all(&destination)?;
    }

    // Avoid copying common files
    let dup_regex = Regex::new(
        r"(\.lock|\.txt|\.woff|\.svg|\.css|main-.*\.css|main-.*\.js|normalize-.*\.js|rustdoc-.*\.css|storage-.*\.js|theme-.*\.js)$")
        .unwrap();

    for file in source.read_dir()? {

        let file = file?;
        let mut destination_full_path = PathBuf::from(&destination);
        destination_full_path.push(file.file_name());

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
    use std::path::Path;
    use super::*;

    #[test]
    fn test_copy_doc_dir() {
        let destination = tempdir::TempDir::new("cratesfyi").unwrap();

        // lets try to copy a src directory to tempdir
        let res = copy_doc_dir(Path::new("src"), destination.path());
        // remove temp dir
        fs::remove_dir_all(destination.path()).unwrap();

        assert!(res.is_ok());
    }
}
