
// FIXME: There is so many PathBuf's in this module
//        Conver them to Path

use std::path::{Path, PathBuf};
use std::fs;
use error::Result;

use regex::Regex;


/// Copies files from source directory to destination directory.
pub fn copy_dir<P: AsRef<Path>>(source: P, destination: P) -> Result<()> {
    copy_files_and_handle_html(source.as_ref().to_path_buf(),
                               destination.as_ref().to_path_buf(),
                               false,
                               "")
}


/// Copies documentation from a crate's target directory to destination.
///
/// Target directory must have doc directory.
///
/// This function is designed to avoid file duplications. It is using rustc version string
/// to rename common files (css files, jquery.js, playpen.js, main.js etc.) in a standard rustdoc.
pub fn copy_doc_dir<P: AsRef<Path>>(target: P,
                                    destination: P,
                                    rustc_version: &str)
                                    -> Result<()> {
    let source = PathBuf::from(target.as_ref()).join("doc");
    copy_files_and_handle_html(source,
                               destination.as_ref().to_path_buf(),
                               true,
                               rustc_version)
}


fn copy_files_and_handle_html(source: PathBuf,
                              destination: PathBuf,
                              handle_html: bool,
                              rustc_version: &str)
                              -> Result<()> {

    // FIXME: handle_html is useless since we started using --resource-suffix
    //        argument with rustdoc

    // Make sure destination directory is exists
    if !destination.exists() {
        fs::create_dir_all(&destination)?;
    }

    // Avoid copying common files
    let dup_regex = Regex::new(
        r"(\.lock|\.txt|\.woff|\.svg|\.css|main-.*\.css|main-.*\.js|normalize-.*\.js|rustdoc-.*\.css|storage-.*\.js|theme-.*\.js)$")
        .unwrap();

    for file in try!(source.read_dir()) {

        let file = file?;
        let mut destination_full_path = PathBuf::from(&destination);
        destination_full_path.push(file.file_name());

        let metadata = file.metadata()?;

        if metadata.is_dir() {
            fs::create_dir_all(&destination_full_path)?;
            copy_files_and_handle_html(file.path(),
                                            destination_full_path,
                                            handle_html,
                                            &rustc_version)?
        } else if handle_html && dup_regex.is_match(&file.file_name().into_string().unwrap()[..]) {
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
    #[ignore]
    fn test_copy_dir() {
        let destination = tempdir::TempDir::new("cratesfyi").unwrap();

        // lets try to copy a src directory to tempdir
        let res = copy_dir(Path::new("src"), destination.path());
        // remove temp dir
        fs::remove_dir_all(destination.path()).unwrap();

        assert!(res.is_ok());
    }
}
