
// FIXME: There is so many PathBuf's in this module
//        Conver them to Path

use std::io::prelude::*;
use std::io;
use std::path::{Path, PathBuf};
use std::fs;
use error::Result;

use regex::Regex;


/// Copies files from source directory to destination directory.
pub fn copy_dir<P: AsRef<Path>>(source: P, destination: P) -> Result<()> {
    copy_files_and_handle_html(source.as_ref().to_path_buf(),
                               destination.as_ref().to_path_buf(),
                               false,
                               "",
                               false)
}


/// Copies documentation from a crate's target directory to destination.
///
/// Target directory must have doc directory.
///
/// This function is designed to avoid file duplications. It is using rustc version string
/// to rename common files (css files, jquery.js, playpen.js, main.js etc.) in a standard rustdoc.
pub fn copy_doc_dir<P: AsRef<Path>>(target: P,
                                    destination: P,
                                    rustc_version: &str,
                                    target_platform: bool)
                                    -> Result<()> {
    let source = PathBuf::from(target.as_ref()).join("doc");
    copy_files_and_handle_html(source,
                               destination.as_ref().to_path_buf(),
                               true,
                               rustc_version,
                               target_platform)
}


fn copy_files_and_handle_html(source: PathBuf,
                              destination: PathBuf,
                              handle_html: bool,
                              rustc_version: &str,
                              target: bool)
                              -> Result<()> {

    // Make sure destination directory is exists
    if !destination.exists() {
        try!(fs::create_dir_all(&destination));
    }

    // Avoid copying duplicated files
    let dup_regex = Regex::new(r"(\.lock|\.txt|\.woff|jquery\.js|playpen\.js|main\.js|\.css)$")
        .unwrap();

    for file in try!(source.read_dir()) {

        let file = try!(file);
        let mut destination_full_path = PathBuf::from(&destination);
        destination_full_path.push(file.file_name());

        let metadata = try!(file.metadata());

        if metadata.is_dir() {
            try!(fs::create_dir_all(&destination_full_path));
            try!(copy_files_and_handle_html(file.path(),
                                            destination_full_path,
                                            handle_html,
                                            &rustc_version,
                                            target));
        } else if handle_html && file.file_name().into_string().unwrap().ends_with(".html") {
            try!(copy_html(&file.path(), &destination_full_path, rustc_version, target));
        } else if handle_html && dup_regex.is_match(&file.file_name().into_string().unwrap()[..]) {
            continue;
        } else {
            try!(fs::copy(&file.path(), &destination_full_path));
        }

    }
    Ok(())
}


fn copy_html(source: &PathBuf,
             destination: &PathBuf,
             rustc_version: &str,
             target: bool)
             -> Result<()> {

    let source_file = try!(fs::File::open(source));
    let mut destination_file = try!(fs::OpenOptions::new()
        .write(true)
        .create(true)
        .open(destination));

    let reader = io::BufReader::new(source_file);

    // FIXME: We don't need to store common libraries (jquery and normalize) for the each version
    //        of rustc. I believe storing only one version of this files should work in every
    //        documentation page.
    let replace_regex =
        Regex::new(r#"(href|src)="(.*)(main|jquery|rustdoc|playpen|normalize)\.(css|js)""#)
            .unwrap();
    let replace_str = format!("$1=\"{}../../$2$3-{}.$4\"",
                              if target { "../" } else { "" },
                              rustc_version);

    for line in reader.lines() {
        let mut line = try!(line);

        // replace css links
        line = replace_regex.replace_all(&line[..], &replace_str[..]).into_owned();

        try!(destination_file.write(line.as_bytes()));
        // need to write consumed newline
        try!(destination_file.write(&['\n' as u8]));
    }

    Ok(())
}



#[cfg(test)]
mod test {
    extern crate env_logger;
    extern crate tempdir;
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


    #[test]
    #[ignore]
    fn test_copy_doc_dir() {
        // lets build documentation of rand crate
        use utils::build_doc;
        let pkg = build_doc("rand", None, None).unwrap();

        let pkg_dir = format!("rand-{}", pkg.manifest().version());
        let target = Path::new(&pkg_dir);
        let destination = tempdir::TempDir::new("cratesfyi").unwrap();
        let res = copy_doc_dir(target, destination.path(), "UNKNOWN", false);

        // remove build and temp dir
        fs::remove_dir_all(target).unwrap();
        fs::remove_dir_all(destination.path()).unwrap();

        assert!(res.is_ok());
    }
}
