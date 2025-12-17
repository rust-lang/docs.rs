use crate::{
    compression::{self, CompressionAlgorithm},
    types::RustdocJsonFormatVersion,
};
use docs_rs_types::Version;

pub fn rustdoc_archive_path(name: &str, version: &Version) -> String {
    format!("rustdoc/{name}/{version}.zip")
}

pub fn rustdoc_json_path(
    name: &str,
    version: &Version,
    target: &str,
    format_version: RustdocJsonFormatVersion,
    compression_algorithm: Option<CompressionAlgorithm>,
) -> String {
    let mut path = format!(
        "rustdoc-json/{name}/{version}/{target}/{name}_{version}_{target}_{format_version}.json"
    );

    if let Some(alg) = compression_algorithm {
        path.push('.');
        path.push_str(compression::file_extension_for(alg));
    }

    path
}

pub fn source_archive_path(name: &str, version: &Version) -> String {
    format!("sources/{name}/{version}.zip")
}
