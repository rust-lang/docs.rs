mod build_status;
mod compression_algorithm;
pub(crate) mod convert;
pub mod doc_coverage;
mod duration;
mod feature;
mod ids;
mod krate_name;
mod req_version;
#[cfg(any(test, feature = "testing"))]
pub mod testing;
mod version;

pub use build_status::BuildStatus;
pub use compression_algorithm::{CompressionAlgorithm, compression_from_file_extension};
pub use doc_coverage::{DocCoverage, RawFileCoverage};
pub use duration::Duration;
pub use feature::Feature;
pub use ids::{BuildId, CrateId, ReleaseId};
pub use krate_name::KrateName;
pub use req_version::ReqVersion;
pub use version::{Version, VersionReq};
