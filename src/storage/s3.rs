use super::{Blob, FileRange, StorageTransaction};
use crate::{Config, InstanceMetrics};
use anyhow::Error;
use aws_sdk_s3::{
    config::{retry::RetryConfig, Region},
    error::SdkError,
    operation::{get_object::GetObjectError, head_object::HeadObjectError},
    types::{Delete, ObjectIdentifier, Tag, Tagging},
    Client,
};
use aws_smithy_types_convert::date_time::DateTimeExt;
use chrono::Utc;
use futures_util::{
    future::TryFutureExt,
    stream::{FuturesUnordered, StreamExt},
};
use std::{io::Write, sync::Arc};
use tokio::runtime::Runtime;
use tracing::{error, warn};

const PUBLIC_ACCESS_TAG: &str = "static-cloudfront-access";
const PUBLIC_ACCESS_VALUE: &str = "allow";

pub(super) struct S3Backend {
    client: Client,
    runtime: Arc<Runtime>,
    bucket: String,
    metrics: Arc<InstanceMetrics>,
    #[cfg(test)]
    temporary: bool,
}

impl S3Backend {
    pub(super) fn new(
        metrics: Arc<InstanceMetrics>,
        config: &Config,
        runtime: Arc<Runtime>,
    ) -> Result<Self, Error> {
        let shared_config = runtime.block_on(aws_config::load_from_env());
        let mut config_builder = aws_sdk_s3::config::Builder::from(&shared_config)
            .retry_config(RetryConfig::standard().with_max_attempts(config.aws_sdk_max_retries))
            .region(Region::new(config.s3_region.clone()));

        if let Some(ref endpoint) = config.s3_endpoint {
            config_builder = config_builder.force_path_style(true).endpoint_url(endpoint);
        }

        let client = Client::from_conf(config_builder.build());

        #[cfg(test)]
        {
            // Create the temporary S3 bucket during tests.
            if config.s3_bucket_is_temporary {
                if cfg!(not(test)) {
                    panic!("safeguard to prevent creating temporary buckets outside of tests");
                }

                runtime.block_on(client.create_bucket().bucket(&config.s3_bucket).send())?;
            }
        }

        Ok(Self {
            client,
            runtime,
            metrics,
            bucket: config.s3_bucket.clone(),
            #[cfg(test)]
            temporary: config.s3_bucket_is_temporary,
        })
    }

    pub(super) fn exists(&self, path: &str) -> Result<bool, Error> {
        self.runtime.block_on(async {
            match self
                .client
                .head_object()
                .bucket(&self.bucket)
                .key(path)
                .send()
                .await
            {
                Ok(_) => Ok(true),
                Err(SdkError::ServiceError(err))
                    if (matches!(err.err(), HeadObjectError::NotFound(_))
                        || err.raw().http().status() == http::StatusCode::NOT_FOUND) =>
                {
                    Ok(false)
                }
                Err(other) => Err(other.into()),
            }
        })
    }

    pub(super) fn get_public_access(&self, path: &str) -> Result<bool, Error> {
        self.runtime.block_on(async {
            match self
                .client
                .get_object_tagging()
                .bucket(&self.bucket)
                .key(path)
                .send()
                .await
            {
                Ok(tags) => Ok(tags
                    .tag_set()
                    .map(|tags| {
                        tags.iter()
                            .filter(|tag| tag.key() == Some(PUBLIC_ACCESS_TAG))
                            .any(|tag| tag.value() == Some(PUBLIC_ACCESS_VALUE))
                    })
                    .unwrap_or(false)),
                Err(SdkError::ServiceError(err)) => {
                    if err.raw().http().status() == http::StatusCode::NOT_FOUND {
                        Err(super::PathNotFoundError.into())
                    } else {
                        Err(err.into_err().into())
                    }
                }
                Err(other) => Err(other.into()),
            }
        })
    }

    pub(super) fn set_public_access(&self, path: &str, public: bool) -> Result<(), Error> {
        self.runtime.block_on(async {
            match self
                .client
                .put_object_tagging()
                .bucket(&self.bucket)
                .key(path)
                .tagging(if public {
                    Tagging::builder()
                        .tag_set(
                            Tag::builder()
                                .key(PUBLIC_ACCESS_TAG)
                                .value(PUBLIC_ACCESS_VALUE)
                                .build(),
                        )
                        .build()
                } else {
                    Tagging::builder().build()
                })
                .send()
                .await
            {
                Ok(_) => Ok(()),
                Err(SdkError::ServiceError(err)) => {
                    if err.raw().http().status() == http::StatusCode::NOT_FOUND {
                        Err(super::PathNotFoundError.into())
                    } else {
                        Err(err.into_err().into())
                    }
                }
                Err(other) => Err(other.into()),
            }
        })
    }

    pub(super) fn get(
        &self,
        path: &str,
        max_size: usize,
        range: Option<FileRange>,
    ) -> Result<Blob, Error> {
        self.runtime.block_on(async {
            let res = self
                .client
                .get_object()
                .bucket(&self.bucket)
                .key(path)
                .set_range(range.map(|r| format!("bytes={}-{}", r.start(), r.end())))
                .send()
                .map_err(|err| match err {
                    SdkError::ServiceError(err)
                        if (matches!(err.err(), GetObjectError::NoSuchKey(_))
                            || err.raw().http().status() == http::StatusCode::NOT_FOUND) =>
                    {
                        super::PathNotFoundError.into()
                    }
                    err => Error::from(err),
                })
                .await?;

            let mut content = crate::utils::sized_buffer::SizedBuffer::new(max_size);
            content.reserve(res.content_length.try_into().ok().unwrap_or(0));

            let mut body = res.body;

            while let Some(data) = body.next().await.transpose()? {
                content.write_all(data.as_ref())?;
            }

            let date_updated = res
                .last_modified
                // This is a bug from AWS, it should always have a modified date of when it was created if nothing else.
                // Workaround it by passing now as the modification time, since the exact time doesn't really matter.
                .and_then(|dt| dt.to_chrono_utc().ok())
                .unwrap_or_else(Utc::now);

            let compression = res.content_encoding.and_then(|s| s.parse().ok());

            Ok(Blob {
                path: path.into(),
                mime: res.content_type.unwrap(),
                date_updated,
                content: content.into_inner(),
                compression,
            })
        })
    }

    pub(super) fn start_storage_transaction(&self) -> S3StorageTransaction {
        S3StorageTransaction { s3: self }
    }

    #[cfg(test)]
    pub(super) fn cleanup_after_test(&self) -> Result<(), Error> {
        if !self.temporary {
            return Ok(());
        }

        if cfg!(not(test)) {
            panic!("safeguard to prevent deleting the production bucket");
        }

        let mut transaction = Box::new(self.start_storage_transaction());
        transaction.delete_prefix("")?;
        transaction.complete()?;

        self.runtime
            .block_on(self.client.delete_bucket().bucket(&self.bucket).send())?;

        Ok(())
    }
}

pub(super) struct S3StorageTransaction<'a> {
    s3: &'a S3Backend,
}

impl<'a> StorageTransaction for S3StorageTransaction<'a> {
    fn store_batch(&mut self, mut batch: Vec<Blob>) -> Result<(), Error> {
        self.s3.runtime.block_on(async {
            // Attempt to upload the batch 3 times
            for _ in 0..3 {
                let mut futures = FuturesUnordered::new();
                for blob in batch.drain(..) {
                    futures.push(
                        self.s3
                            .client
                            .put_object()
                            .bucket(&self.s3.bucket)
                            .key(&blob.path)
                            .body(blob.content.clone().into())
                            .content_type(&blob.mime)
                            .set_content_encoding(blob.compression.map(|alg| alg.to_string()))
                            .send()
                            .map_ok(|_| {
                                self.s3.metrics.uploaded_files_total.inc();
                            })
                            .map_err(|err| {
                                warn!("Failed to upload blob to S3: {:?}", err);
                                // Reintroduce failed blobs for a retry
                                blob
                            }),
                    );
                }

                while let Some(result) = futures.next().await {
                    // Push each failed blob back into the batch
                    if let Err(blob) = result {
                        batch.push(blob);
                    }
                }

                // If we uploaded everything in the batch, we're done
                if batch.is_empty() {
                    return Ok(());
                }
            }

            panic!("failed to upload 3 times, exiting");
        })
    }

    fn delete_prefix(&mut self, prefix: &str) -> Result<(), Error> {
        self.s3.runtime.block_on(async {
            let mut continuation_token = None;
            loop {
                let list = self
                    .s3
                    .client
                    .list_objects_v2()
                    .bucket(&self.s3.bucket)
                    .prefix(prefix)
                    .set_continuation_token(continuation_token)
                    .send()
                    .await?;

                if list.key_count() > 0 {
                    let to_delete = Delete::builder()
                        .set_objects(Some(
                            list.contents
                                .expect("didn't get content even though key_count was > 0")
                                .into_iter()
                                .filter_map(|obj| {
                                    obj.key()
                                        .map(|k| ObjectIdentifier::builder().key(k).build())
                                })
                                .collect(),
                        ))
                        .build();

                    let resp = self
                        .s3
                        .client
                        .delete_objects()
                        .bucket(&self.s3.bucket)
                        .delete(to_delete)
                        .send()
                        .await?;

                    if let Some(errs) = resp.errors {
                        for err in &errs {
                            error!("error deleting file from s3: {:?}", err);
                        }

                        anyhow::bail!("deleting from s3 failed");
                    }
                }

                continuation_token = list.next_continuation_token;
                if continuation_token.is_none() {
                    return Ok(());
                }
            }
        })
    }

    fn complete(self: Box<Self>) -> Result<(), Error> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    // The tests for this module are in src/storage/mod.rs, as part of the backend tests. Please
    // add any test checking the public interface there.

    // NOTE: trying to upload a file ending with `/` will behave differently in test and prod.
    // NOTE: On s3, it will succeed and create a file called `/`.
    // NOTE: On min.io, it will fail with 'Object name contains unsupported characters.'
}
