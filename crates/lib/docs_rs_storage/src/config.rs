use crate::types::StorageKind;
use docs_rs_config::AppConfig;
use docs_rs_env_vars::{env, maybe_env, require_env};
use std::{
    io,
    path::{self, Path, PathBuf},
    sync::Arc,
    time::Duration,
};

fn ensure_absolute_path(path: PathBuf) -> io::Result<PathBuf> {
    if path.is_absolute() {
        Ok(path)
    } else {
        Ok(path::absolute(&path)?)
    }
}

#[derive(Debug)]
pub struct ArchiveIndexCacheConfig {
    // where do we want to store the locally cached index files
    // for the remote archives?
    pub path: PathBuf,

    // maximum disk space for the local archive index cache.
    pub max_size_mb: u64,

    // TTL for the local index cache
    pub ttl: Duration,

    // expected number of entries in the local archive cache.
    // Makes server restarts faster by preallocating some data structures.
    // General numbers (as of 2025-12):
    // * we have ~1.5 mio releases with archive storage (and 400k without)
    // * each release has on average 2 archive files (rustdoc, source)
    // so, over all, 3 mio archive index files in S3.
    //
    // While due to crawlers we will download _all_ of them over time, the old
    // metric "releases accessed in the last 10 minutes" was around 50k, if I
    // recall correctly.
    // We use this to pre-allocate the in-memory cache so it can avoid
    // resizes during early traffic.
    pub expected_count: usize,
}

impl AppConfig for ArchiveIndexCacheConfig {
    fn from_environment() -> anyhow::Result<Self> {
        let prefix: PathBuf = require_env("DOCSRS_PREFIX")?;
        Ok(Self {
            path: ensure_absolute_path(env(
                "DOCSRS_ARCHIVE_INDEX_CACHE_PATH",
                prefix.join("archive_cache"),
            )?)?,
            max_size_mb: env(
                "DOCSRS_ARCHIVE_INDEX_CACHE_MAX_SIZE_MB",
                50 * 1024, // 50 GiB
            )?,
            ttl: Duration::from_secs(env(
                "DOCSRS_ARCHIVE_INDEX_CACHE_TTL",
                24 * 60 * 60, // 24 hours
            )?),
            expected_count: env("DOCSRS_ARCHIVE_INDEX_EXPECTED_COUNT", 100_000usize)?,
        })
    }

    #[cfg(any(feature = "testing", test))]
    fn test_config() -> anyhow::Result<Self> {
        let mut config = Self::from_environment()?;
        config.path =
            std::env::temp_dir().join(format!("docsrs-test-index-{}", rand::random::<u64>()));

        Ok(config)
    }
}

#[derive(Debug)]
pub struct Config {
    // Storage params
    pub storage_backend: StorageKind,

    // AWS SDK configuration
    pub aws_sdk_max_retries: u32,

    // S3 params
    pub s3_bucket: String,
    pub s3_region: String,
    pub s3_endpoint: Option<String>,

    // DO NOT CONFIGURE THIS THROUGH AN ENVIRONMENT VARIABLE!
    // Accidentally turning this on outside of the test suite might cause data loss in the
    // production environment.
    #[cfg(any(test, feature = "testing"))]
    pub s3_bucket_is_temporary: bool,

    // Max size of the files served by the docs.rs frontend
    pub max_file_size: usize,
    pub max_file_size_html: usize,

    // config for the local archive index cache
    pub archive_index_cache: Arc<ArchiveIndexCacheConfig>,

    // How much we want to parallelize local filesystem logic.
    // For pure I/O this could be quite high (32/64), but
    // we often also add compression on top of it, which is CPU-bound,
    // even when just light / simpler compression.
    pub local_filesystem_parallelism: usize,

    // How much we want to parallelize file uploads / downloads.
    pub network_parallelism: usize,
}

impl AppConfig for Config {
    fn from_environment() -> anyhow::Result<Self> {
        let cores = std::thread::available_parallelism()?.get();

        Ok(Self {
            storage_backend: env("DOCSRS_STORAGE_BACKEND", StorageKind::default())?,
            aws_sdk_max_retries: env("DOCSRS_AWS_SDK_MAX_RETRIES", 6u32)?,
            s3_bucket: env("DOCSRS_S3_BUCKET", "rust-docs-rs".to_string())?,
            s3_region: env("S3_REGION", "us-west-1".to_string())?,
            s3_endpoint: maybe_env("S3_ENDPOINT")?,
            archive_index_cache: Arc::new(ArchiveIndexCacheConfig::from_environment()?),
            max_file_size: env("DOCSRS_MAX_FILE_SIZE", 50 * 1024 * 1024)?,
            max_file_size_html: env("DOCSRS_MAX_FILE_SIZE_HTML", 50 * 1024 * 1024)?,
            #[cfg(any(test, feature = "testing"))]
            s3_bucket_is_temporary: false,
            local_filesystem_parallelism: env("DOCSRS_LOCAL_FILESYSTEM_PARALLELISM", cores)?,
            network_parallelism: env("DOCSRS_NETWORK_PARALLELISM", 8usize.min(cores))?,
        })
    }

    #[cfg(any(feature = "testing", test))]
    fn test_config() -> anyhow::Result<Self> {
        Self::test_config_with_kind(StorageKind::Memory)
    }
}

impl Config {
    #[cfg(any(feature = "testing", test))]
    pub fn set<F>(self, f: F) -> Self
    where
        F: FnOnce(Self) -> Self,
    {
        f(self)
    }

    pub fn max_file_size_for(&self, path: impl AsRef<Path>) -> usize {
        static HTML: &str = "html";

        if let Some(ext) = path.as_ref().extension()
            && ext == HTML
        {
            self.max_file_size_html
        } else {
            self.max_file_size
        }
    }

    #[cfg(any(feature = "testing", test))]
    pub fn test_config_with_kind(kind: StorageKind) -> anyhow::Result<Self> {
        let mut config = Self::from_environment()?;
        config.storage_backend = kind;

        config.archive_index_cache = Arc::new(ArchiveIndexCacheConfig::test_config()?);

        // Use a temporary S3 bucket, only used when storage_kind is set to S3 in env or later.
        config.s3_bucket = format!("docsrs-test-bucket-{}", rand::random::<u64>());
        config.s3_bucket_is_temporary = true;

        Ok(config)
    }
}
