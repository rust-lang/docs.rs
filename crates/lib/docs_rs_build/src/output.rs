use docsrs_metadata::Metadata;
use rustwide::Build;
use std::path::PathBuf;

/// Name of rustdoc's documentation output directory.
pub const DOC_OUTPUT_DIR_NAME: &str = "doc";

/// Return the host path containing documentation for a target.
///
/// Cargo places proc-macro documentation in the host target directory even
/// when a target argument is otherwise in use.
pub fn doc_output_dir(build: &Build<'_>, metadata: &Metadata, target: &str) -> PathBuf {
    if metadata.proc_macro {
        build.host_target_dir().join(DOC_OUTPUT_DIR_NAME)
    } else {
        build
            .host_target_dir()
            .join(target)
            .join(DOC_OUTPUT_DIR_NAME)
    }
}
