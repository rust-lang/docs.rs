mod limits;
mod rustwide_builder;

pub(crate) use self::limits::Limits;
pub(crate) use self::rustwide_builder::DocCoverage;
pub use self::rustwide_builder::{
    BuildPackageSummary, BuilderMetrics, PackageKind, RustwideBuilder,
};

#[cfg(test)]
pub use self::rustwide_builder::{
    RUSTDOC_JSON_COMPRESSION_ALGORITHMS, read_format_version_from_rustdoc_json,
};
