use crate::docbuilder::DOC_OUTPUT_DIR_NAME;
use anyhow::{Result, bail};
use docsrs_metadata::Metadata;
use rustwide::Build;
use std::{
    ffi::OsString,
    fs,
    path::{Path, PathBuf},
};

pub(crate) trait RustwideBuildExt {
    /// get the documentation output directory for the build.
    ///
    /// Normally `./target/{doc_target}/doc`, or `./target/doc` for proc-macro libs.
    fn doc_output_dir(&self, metadata: &Metadata, target: impl AsRef<str>) -> PathBuf;
}

impl RustwideBuildExt for Build<'_> {
    fn doc_output_dir(&self, metadata: &Metadata, target: impl AsRef<str>) -> PathBuf {
        if metadata.proc_macro {
            self.host_target_dir().join(DOC_OUTPUT_DIR_NAME)
        } else {
            self.host_target_dir()
                .join(target.as_ref())
                .join(DOC_OUTPUT_DIR_NAME)
        }
    }
}

/// find a single file in the doc build output.
///
/// For
/// * rustdoc json
/// * coverage
///
/// where the directory is emptied, and we then get exactly one
/// file.
pub(crate) fn find_single_file_in_doc_output_dir(
    doc_output_dir: impl AsRef<Path>,
    ext: impl Into<OsString>,
) -> Result<PathBuf> {
    let doc_output_dir = doc_output_dir.as_ref();
    let ext = ext.into();

    let folder_contents: Vec<_> = fs::read_dir(doc_output_dir)?
        .filter_map(|entry| {
            let entry = entry.ok()?;
            if !entry.file_type().ok()?.is_file() {
                return None;
            }

            let path = entry.path();
            if path.extension()?.eq_ignore_ascii_case(&ext) {
                Some(path)
            } else {
                None
            }
        })
        .collect();

    if folder_contents.len() != 1 {
        bail!(
            "found {} instead of exactly one {} file in target/doc after build.\n\
                     search directory: {}\n\
                     files: {:?}",
            folder_contents.len(),
            ext.to_string_lossy(),
            doc_output_dir.to_string_lossy(),
            folder_contents,
        );
    }

    Ok(folder_contents
        .into_iter()
        .next()
        .expect("length checked above"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn find_single_file_in_doc_output_finds_a_matching_file() -> Result<()> {
        let tempdir = tempfile::tempdir()?;
        let output_dir = tempdir.path().join(DOC_OUTPUT_DIR_NAME);
        fs::create_dir_all(&output_dir)?;
        let expected = output_dir.join("crate.JSON");
        fs::write(&expected, "{}")?;
        fs::write(output_dir.join("index.html"), "")?;

        assert_eq!(
            find_single_file_in_doc_output_dir(&output_dir, "json")?,
            expected
        );

        Ok(())
    }

    #[test]
    fn find_single_file_in_doc_output_errors_without_a_match() -> Result<()> {
        let tempdir = tempfile::tempdir()?;
        let output_dir = tempdir.path().join(DOC_OUTPUT_DIR_NAME);
        fs::create_dir_all(&output_dir)?;
        fs::write(output_dir.join("index.html"), "")?;

        let err = find_single_file_in_doc_output_dir(&output_dir, "json")
            .unwrap_err()
            .to_string();

        assert!(err.contains("found 0 instead of exactly one json file"));
        assert!(err.contains(&output_dir.to_string_lossy().to_string()));
        Ok(())
    }

    #[test]
    fn find_single_file_in_doc_output_errors_with_multiple_matches() -> Result<()> {
        let tempdir = tempfile::tempdir()?;
        let output_dir = tempdir.path().join(DOC_OUTPUT_DIR_NAME);
        fs::create_dir_all(&output_dir)?;
        fs::write(output_dir.join("first.json"), "{}")?;
        fs::write(output_dir.join("second.json"), "{}")?;

        let err = find_single_file_in_doc_output_dir(&output_dir, "json")
            .unwrap_err()
            .to_string();

        assert!(err.contains("found 2 instead of exactly one json file"));
        assert!(err.contains(&output_dir.to_string_lossy().to_string()));
        Ok(())
    }
}
