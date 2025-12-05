use crate::StorageKind;
use docs_rs_env_vars::{env, maybe_env, require_env};
use std::{
    io,
    path::{self, Path, PathBuf},
};

fn ensure_absolute_path(path: PathBuf) -> io::Result<PathBuf> {
    if path.is_absolute() {
        Ok(path)
    } else {
        Ok(path::absolute(&path)?)
    }
}

#[derive(Debug)]
pub struct Config {
    pub temp_dir: PathBuf,

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
    #[cfg(test)]
    pub s3_bucket_is_temporary: bool,

    // Max size of the files served by the docs.rs frontend
    pub max_file_size: usize,
    pub max_file_size_html: usize,

    // where do we want to store the locally cached index files
    // for the remote archives?
    pub local_archive_cache_path: PathBuf,

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
    // We're using a local DashMap to store some locks for these indexes,
    // and we already know in advance we need these 50k entries.
    // So we can preallocate the DashMap with this number to avoid resizes.
    pub local_archive_cache_expected_count: usize,
}

impl Config {
    pub fn from_environment() -> anyhow::Result<Self> {
        let prefix: PathBuf = require_env("DOCSRS_PREFIX")?;

        Ok(Self {
            temp_dir: prefix.join("tmp"),
            storage_backend: env("DOCSRS_STORAGE_BACKEND", StorageKind::Database)?,
            aws_sdk_max_retries: env("DOCSRS_AWS_SDK_MAX_RETRIES", 6u32)?,
            s3_bucket: env("DOCSRS_S3_BUCKET", "rust-docs-rs".to_string())?,
            s3_region: env("S3_REGION", "us-west-1".to_string())?,
            s3_endpoint: maybe_env("S3_ENDPOINT")?,
            local_archive_cache_path: ensure_absolute_path(env(
                "DOCSRS_ARCHIVE_INDEX_CACHE_PATH",
                prefix.join("archive_cache"),
            )?)?,
            local_archive_cache_expected_count: env(
                "DOCSRS_ARCHIVE_INDEX_EXPECTED_COUNT",
                100_000usize,
            )?,
            max_file_size: env("DOCSRS_MAX_FILE_SIZE", 50 * 1024 * 1024)?,
            max_file_size_html: env("DOCSRS_MAX_FILE_SIZE_HTML", 50 * 1024 * 1024)?,
            #[cfg(test)]
            s3_bucket_is_temporary: false,
        })
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
}
