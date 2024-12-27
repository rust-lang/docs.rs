use super::{Blob, FileRange};
use crate::{Config, InstanceMetrics};
use anyhow::{Context as _, Error};
use async_stream::try_stream;
use aws_config::BehaviorVersion;
use aws_sdk_s3::{
    config::{retry::RetryConfig, Region},
    error::{ProvideErrorMetadata, SdkError},
    types::{Delete, ObjectIdentifier, Tag, Tagging},
    Client,
};
use aws_smithy_types_convert::date_time::DateTimeExt;
use chrono::Utc;
use futures_util::{
    future::TryFutureExt,
    pin_mut,
    stream::{FuturesUnordered, Stream, StreamExt},
};
use std::{io::Write, sync::Arc};
use tracing::{error, warn};

const PUBLIC_ACCESS_TAG: &str = "static-cloudfront-access";
const PUBLIC_ACCESS_VALUE: &str = "allow";

// error codes to check for when trying to determine if an error is
// a "NOT FOUND" error.
// Definition taken from the S3 rust SDK,
// and validated by manually testing with actual S3.
static NOT_FOUND_ERROR_CODES: [&str; 5] = [
    // from sentry errors
    "KeyTooLongError",
    // https://github.com/awslabs/aws-sdk-rust/blob/6155192dbd003af7bc5d4c1419bf17794302f8c3/sdk/s3/src/protocol_serde/shape_get_object.rs#L201-L251
    "NoSuchKey",
    // https://github.com/awslabs/aws-sdk-rust/blob/6155192dbd003af7bc5d4c1419bf17794302f8c3/sdk/s3/src/protocol_serde/shape_head_object.rs#L1-L39"
    "NotFound",
    // https://github.com/awslabs/aws-sdk-rust/blob/6155192dbd003af7bc5d4c1419bf17794302f8c3/sdk/mediastoredata/src/protocol_serde/shape_get_object.rs#L47-L128
    "ObjectNotFoundException",
    // from testing long keys with minio
    "XMinioInvalidObjectName",
];

trait S3ResultExt<T> {
    fn convert_errors(self) -> anyhow::Result<T>;
}

impl<T, E> S3ResultExt<T> for Result<T, SdkError<E>>
where
    E: ProvideErrorMetadata + std::error::Error + Send + Sync + 'static,
{
    fn convert_errors(self) -> anyhow::Result<T> {
        match self {
            Ok(result) => Ok(result),
            Err(err) => {
                if let Some(err_code) = err.code() {
                    if NOT_FOUND_ERROR_CODES.iter().any(|&code| err_code == code) {
                        return Err(super::PathNotFoundError.into());
                    }
                }

                if let SdkError::ServiceError(err) = &err {
                    if err.raw().status().as_u16() == http::StatusCode::NOT_FOUND.as_u16() {
                        return Err(super::PathNotFoundError.into());
                    }
                }

                Err(err.into())
            }
        }
    }
}

pub(super) struct S3Backend {
    client: Client,
    bucket: String,
    metrics: Arc<InstanceMetrics>,
    #[cfg(test)]
    temporary: bool,
}

impl S3Backend {
    pub(super) async fn new(metrics: Arc<InstanceMetrics>, config: &Config) -> Result<Self, Error> {
        let shared_config = aws_config::load_defaults(BehaviorVersion::latest()).await;
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

                client
                    .create_bucket()
                    .bucket(&config.s3_bucket)
                    .send()
                    .await?;
            }
        }

        Ok(Self {
            client,
            metrics,
            bucket: config.s3_bucket.clone(),
            #[cfg(test)]
            temporary: config.s3_bucket_is_temporary,
        })
    }

    pub(super) async fn exists(&self, path: &str) -> Result<bool, Error> {
        match self
            .client
            .head_object()
            .bucket(&self.bucket)
            .key(path)
            .send()
            .await
            .convert_errors()
        {
            Ok(_) => Ok(true),
            Err(err) if err.is::<super::PathNotFoundError>() => Ok(false),
            Err(other) => Err(other),
        }
    }

    pub(super) async fn get_public_access(&self, path: &str) -> Result<bool, Error> {
        Ok(self
            .client
            .get_object_tagging()
            .bucket(&self.bucket)
            .key(path)
            .send()
            .await
            .convert_errors()?
            .tag_set()
            .iter()
            .filter(|tag| tag.key() == PUBLIC_ACCESS_TAG)
            .any(|tag| tag.value() == PUBLIC_ACCESS_VALUE))
    }

    pub(super) async fn set_public_access(&self, path: &str, public: bool) -> Result<(), Error> {
        self.client
            .put_object_tagging()
            .bucket(&self.bucket)
            .key(path)
            .tagging(if public {
                Tagging::builder()
                    .tag_set(
                        Tag::builder()
                            .key(PUBLIC_ACCESS_TAG)
                            .value(PUBLIC_ACCESS_VALUE)
                            .build()
                            .context("could not build tag")?,
                    )
                    .build()
                    .context("could not build tags")?
            } else {
                Tagging::builder()
                    .set_tag_set(Some(vec![]))
                    .build()
                    .context("could not build tags")?
            })
            .send()
            .await
            .convert_errors()
            .map(|_| ())
    }

    pub(super) async fn get(
        &self,
        path: &str,
        max_size: usize,
        range: Option<FileRange>,
    ) -> Result<Blob, Error> {
        let res = self
            .client
            .get_object()
            .bucket(&self.bucket)
            .key(path)
            .set_range(range.map(|r| format!("bytes={}-{}", r.start(), r.end())))
            .send()
            .await
            .convert_errors()?;

        let mut content = crate::utils::sized_buffer::SizedBuffer::new(max_size);
        content.reserve(
            res.content_length
                .and_then(|length| length.try_into().ok())
                .unwrap_or(0),
        );

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
            mime: res
                .content_type
                .as_ref()
                .unwrap()
                .parse()
                .unwrap_or(mime::APPLICATION_OCTET_STREAM),
            date_updated,
            content: content.into_inner(),
            compression,
        })
    }

    pub(super) async fn store_batch(&self, mut batch: Vec<Blob>) -> Result<(), Error> {
        // Attempt to upload the batch 3 times
        for _ in 0..3 {
            let mut futures = FuturesUnordered::new();
            for blob in batch.drain(..) {
                futures.push(
                    self.client
                        .put_object()
                        .bucket(&self.bucket)
                        .key(&blob.path)
                        .body(blob.content.clone().into())
                        .content_type(blob.mime.to_string())
                        .set_content_encoding(blob.compression.map(|alg| alg.to_string()))
                        .send()
                        .map_ok(|_| {
                            self.metrics.uploaded_files_total.inc();
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
    }

    pub(super) async fn list_prefix<'a>(
        &'a self,
        prefix: &'a str,
    ) -> impl Stream<Item = Result<String, Error>> + 'a {
        try_stream! {
            let mut continuation_token = None;
            loop {
                let list = self
                    .client
                    .list_objects_v2()
                    .bucket(&self.bucket)
                    .prefix(prefix)
                    .set_continuation_token(continuation_token)
                    .send()
                    .await?;

                if let Some(contents) = list.contents {
                    for obj in contents {
                        if let Some(key) = obj.key() {
                            yield key.to_owned();
                        }
                    }
                }

                continuation_token = list.next_continuation_token;
                if continuation_token.is_none() {
                    break;
                }
            }
        }
    }

    pub(super) async fn delete_prefix(&self, prefix: &str) -> Result<(), Error> {
        let stream = self.list_prefix(prefix).await;
        pin_mut!(stream);
        let mut chunks = stream.chunks(900); // 1000 is the limit for the delete_objects API

        while let Some(batch) = chunks.next().await {
            let batch: Vec<_> = batch.into_iter().collect::<anyhow::Result<_>>()?;

            let to_delete = Delete::builder()
                .set_objects(Some(
                    batch
                        .into_iter()
                        .filter_map(|k| ObjectIdentifier::builder().key(k).build().ok())
                        .collect(),
                ))
                .build()
                .context("could not build delete request")?;

            let resp = self
                .client
                .delete_objects()
                .bucket(&self.bucket)
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
        Ok(())
    }

    #[cfg(test)]
    pub(super) async fn cleanup_after_test(&self) -> Result<(), Error> {
        if !self.temporary {
            return Ok(());
        }

        if cfg!(not(test)) {
            panic!("safeguard to prevent deleting the production bucket");
        }

        self.delete_prefix("").await?;
        self.client
            .delete_bucket()
            .bucket(&self.bucket)
            .send()
            .await?;

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
