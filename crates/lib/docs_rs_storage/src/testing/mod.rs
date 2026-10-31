mod test_env;
mod utils;

pub use test_env::TestStorage;
pub use utils::check_archive_consistency;

/// Open another connection to a local archive index for tests and benchmarks.
pub async fn open_archive_index(
    index_path: &std::path::Path,
) -> anyhow::Result<crate::archive_index::Index> {
    crate::archive_index::Index::open(index_path).await
}
