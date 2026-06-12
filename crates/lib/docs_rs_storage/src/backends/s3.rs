use crate::{
    Config,
    backends::StorageBackendMethods,
    blob::{StreamUpload, StreamUploadSource, StreamingBlob},
    crc32_for_path,
    errors::PathNotFoundError,
    metrics::{StorageMetrics, UploadType},
    types::FileRange,
    utils::crc32::crc32_for_path_range,
};
use anyhow::{Context as _, Error};
use async_stream::try_stream;
use aws_config::BehaviorVersion;
use aws_sdk_s3::{
    Client,
    config::{Region, retry::RetryConfig},
    error::{ProvideErrorMetadata, SdkError},
    primitives::{ByteStream, Length},
    types::{ChecksumAlgorithm, CompletedMultipartUpload, CompletedPart, Delete, ObjectIdentifier},
};
use aws_smithy_types_convert::date_time::DateTimeExt;
use base64::{Engine as _, engine::general_purpose::STANDARD as b64};
use chrono::Utc;
use docs_rs_headers::{ETag, compute_etag};
use docs_rs_types::CompressionAlgorithm;
use docs_rs_utils::spawn_blocking;
use futures_util::stream::{self, BoxStream, StreamExt, TryStreamExt};
use mime::Mime;
use std::path::Path;
use tokio::fs;
use tracing::error;

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

const S3_UPLOAD_BUFFER_SIZE: usize = 1024 * 1024; // 1 MiB
// AWS recommends multipart uploads for > 100 MiB.
// normal uploads only work up to 5 GiB.
const S3_MULTIPART_UPLOAD_THRESHOLD: u64 = 100 * 1024 * 1024; // 100 MiB
const S3_MULTIPART_PART_SIZE: u64 = S3_MULTIPART_UPLOAD_THRESHOLD; // 100 MiB

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
                if let Some(err_code) = err.code()
                    && NOT_FOUND_ERROR_CODES.contains(&err_code)
                {
                    return Err(PathNotFoundError.into());
                }

                if let SdkError::ServiceError(err) = &err
                    && err.raw().status().as_u16() == http::StatusCode::NOT_FOUND.as_u16()
                {
                    return Err(PathNotFoundError.into());
                }

                Err(err.into())
            }
        }
    }
}

pub(crate) struct S3Backend {
    client: Client,
    bucket: String,
    otel_metrics: StorageMetrics,
    network_parallelism: usize,
    #[cfg(any(test, feature = "testing"))]
    temporary: bool,
}

impl S3Backend {
    pub(crate) async fn new(config: &Config, otel_metrics: StorageMetrics) -> Result<Self, Error> {
        let shared_config = aws_config::load_defaults(BehaviorVersion::latest()).await;
        let mut config_builder = aws_sdk_s3::config::Builder::from(&shared_config)
            .retry_config(RetryConfig::standard().with_max_attempts(config.aws_sdk_max_retries))
            .region(Region::new(config.s3_region.clone()));

        if let Some(ref endpoint) = config.s3_endpoint {
            config_builder = config_builder.force_path_style(true).endpoint_url(endpoint);
        }

        let client = Client::from_conf(config_builder.build());

        #[cfg(any(test, feature = "testing"))]
        {
            // Create the temporary S3 bucket during tests.
            if config.s3_bucket_is_temporary {
                client
                    .create_bucket()
                    .bucket(&config.s3_bucket)
                    .send()
                    .await?;
            }
        }

        Ok(Self {
            client,
            otel_metrics,
            bucket: config.s3_bucket.clone(),
            network_parallelism: config.network_parallelism,
            #[cfg(any(test, feature = "testing"))]
            temporary: config.s3_bucket_is_temporary,
        })
    }

    #[cfg(any(test, feature = "testing"))]
    pub(crate) async fn cleanup_after_test(&self) -> Result<(), Error> {
        assert!(
            self.temporary,
            "cleanup_after_test called on non-temporary S3 backend"
        );

        self.delete_prefix("").await?;
        self.client
            .delete_bucket()
            .bucket(&self.bucket)
            .send()
            .await?;

        Ok(())
    }

    /// upload a stream to S3, in a single `put_object` API call.
    ///
    /// Works only for files up to 5 GiB.
    async fn upload_stream_single(
        &self,
        path: &str,
        mime: &Mime,
        source: &StreamUploadSource,
        compression: Option<CompressionAlgorithm>,
        content_length: u64,
        checksum_crc32: Option<&str>,
    ) -> Result<(), Error> {
        let body = match source {
            StreamUploadSource::Bytes(bytes) => ByteStream::from(bytes.clone()),
            StreamUploadSource::File(path) => {
                // NOTE:
                // reading the upload-data from a local path is
                // "retryable" in the AWS SDK sense.
                // ".file" (file pointer) is not retryable.
                ByteStream::read_from()
                    .path(path)
                    .buffer_size(S3_UPLOAD_BUFFER_SIZE)
                    .length(Length::Exact(content_length))
                    .build()
                    .await?
            }
        };

        let mut request = self
            .client
            .put_object()
            .bucket(&self.bucket)
            .key(path)
            .body(body)
            .content_length(content_length as i64)
            .content_type(mime.to_string())
            .set_content_encoding(compression.map(|alg| alg.to_string()));

        // NOTE: when you try to stream-upload a local file, the AWS SDK by default
        // uses a "middleware" to calculate the checksum for the content, to compare it after
        // uploading.
        // This piece is broken right now, but only when using S3 directly. On minio, all is
        // fine.
        // I don't want to disable checksums so we're sure the files are uploaded correctly.
        // So the only alternative (outside of trying to fix the SDK) is to calculate the
        // checksum ourselves. This is a little annoying because this means we have to read the
        // whole file before upload. But since I don't want to load all files into memory before
        // upload, this is the only option.
        if let Some(checksum_crc32) = checksum_crc32 {
            request = request
                .checksum_algorithm(ChecksumAlgorithm::Crc32)
                .checksum_crc32(checksum_crc32);
        }

        request.send().await?;

        self.otel_metrics
            .record_upload_metrics(content_length, UploadType::Single);

        Ok(())
    }

    /// upload a stream to S3, as multipart upload.
    ///
    /// Better for bigger files because we can split the file into parts,
    /// and then upload & retry them separately.
    ///
    /// Not so good for small files, because we have more API calls.
    async fn upload_file_multipart(
        &self,
        path: &str,
        mime: &Mime,
        local_path: &Path,
        compression: Option<CompressionAlgorithm>,
        content_length: u64,
    ) -> Result<(), Error> {
        let upload_id = self
            .client
            .create_multipart_upload()
            .bucket(&self.bucket)
            .key(path)
            .content_type(mime.to_string())
            .checksum_algorithm(ChecksumAlgorithm::Crc32)
            .set_content_encoding(compression.map(|alg| alg.to_string()))
            .send()
            .await?
            .upload_id
            .context("S3 did not return an upload ID")?;

        let result = async {
            let parts = self
                .upload_file_multipart_parts(path, local_path, &upload_id, content_length)
                .await?;

            let completed_upload = CompletedMultipartUpload::builder()
                .set_parts(Some(parts))
                .build();

            self.client
                .complete_multipart_upload()
                .bucket(&self.bucket)
                .key(path)
                .upload_id(&upload_id)
                .multipart_upload(completed_upload)
                .send()
                .await?;

            Ok::<_, Error>(())
        }
        .await;

        match result {
            Ok(()) => {
                self.otel_metrics
                    .record_upload_metrics(content_length, UploadType::MultiPart);
                Ok(())
            }
            Err(err) => {
                if let Err(abort_err) = self
                    .client
                    .abort_multipart_upload()
                    .bucket(&self.bucket)
                    .key(path)
                    .upload_id(&upload_id)
                    .send()
                    .await
                {
                    error!(
                        ?abort_err,
                        %path,
                        upload_id,
                        "failed to abort multipart upload after upload error"
                    );
                }

                Err(err)
            }
        }
    }

    async fn upload_file_multipart_parts(
        &self,
        path: &str,
        local_path: &Path,
        upload_id: &str,
        content_length: u64,
    ) -> Result<Vec<CompletedPart>, Error> {
        let part_count = content_length.div_ceil(S3_MULTIPART_PART_SIZE);
        let mut parts = stream::iter(0..part_count)
            .map(|part_index| {
                self.upload_file_multipart_part(
                    path,
                    local_path,
                    upload_id,
                    part_index,
                    content_length,
                )
            })
            .buffer_unordered(self.network_parallelism)
            .try_collect::<Vec<_>>()
            .await?;

        parts.sort_by_key(|part| part.part_number.unwrap_or_default());

        Ok(parts)
    }

    async fn upload_file_multipart_part(
        &self,
        path: &str,
        local_path: &Path,
        upload_id: &str,
        part_index: u64,
        content_length: u64,
    ) -> Result<CompletedPart, Error> {
        let offset = part_index * S3_MULTIPART_PART_SIZE;
        let part_length = (content_length - offset).min(S3_MULTIPART_PART_SIZE);
        let part_number = (part_index + 1) as i32;
        let checksum_crc32 = spawn_blocking({
            let local_path = local_path.to_path_buf();
            move || Ok(b64.encode(crc32_for_path_range(local_path, offset, part_length)?))
        })
        .await?;

        let body = ByteStream::read_from()
            .path(local_path)
            .buffer_size(S3_UPLOAD_BUFFER_SIZE)
            .offset(offset)
            .length(Length::Exact(part_length))
            .build()
            .await?;

        let output = self
            .client
            .upload_part()
            .bucket(&self.bucket)
            .key(path)
            .upload_id(upload_id)
            .part_number(part_number)
            .body(body)
            .content_length(part_length as i64)
            .checksum_algorithm(ChecksumAlgorithm::Crc32)
            .checksum_crc32(&checksum_crc32)
            .send()
            .await?;

        Ok(CompletedPart::builder()
            .part_number(part_number)
            .set_e_tag(output.e_tag)
            .checksum_crc32(&checksum_crc32)
            .build())
    }
}

impl StorageBackendMethods for S3Backend {
    async fn exists(&self, path: &str) -> Result<bool, Error> {
        self.otel_metrics.exist_calls.add(1, &[]);

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
            Err(err) if err.is::<PathNotFoundError>() => Ok(false),
            Err(other) => Err(other),
        }
    }

    async fn get_stream(
        &self,
        path: &str,
        range: Option<FileRange>,
    ) -> Result<StreamingBlob, Error> {
        let res = self
            .client
            .get_object()
            .bucket(&self.bucket)
            .key(path)
            .set_range(
                range
                    .as_ref()
                    .map(|r| format!("bytes={}-{}", r.start(), r.end())),
            )
            .send()
            .await
            .convert_errors()?;

        let date_updated = res
            .last_modified
            // This is a bug from AWS, it should always have a modified date of when it was created if nothing else.
            // Workaround it by passing now as the modification time, since the exact time doesn't really matter.
            .and_then(|dt| dt.to_chrono_utc().ok())
            .unwrap_or_else(Utc::now);

        let compression = res.content_encoding.as_ref().and_then(|s| s.parse().ok());

        let etag = if let Some(s3_etag) = res.e_tag
            && !s3_etag.is_empty()
        {
            if let Some(range) = &range {
                // we can generate a unique etag for a range of the remote object too,
                // by just concatenating the original etag with the range start and end.
                //
                // About edge cases:
                // When the etag of the full archive changes after a rebuild,
                // all derived etags for files inside the archive will also change.
                //
                // This could lead to _changed_ ETags, where the single file inside the archive
                // is actually the same.
                //
                // AWS implementation (an minio) is to just use an MD5 hash of the file as
                // ETag
                Some(compute_etag(format!(
                    "{}-{}-{}",
                    s3_etag,
                    range.start(),
                    range.end()
                )))
            } else {
                match s3_etag.parse::<ETag>() {
                    Ok(etag) => Some(etag),
                    Err(err) => {
                        error!(?err, s3_etag, "Failed to parse ETag from S3");
                        None
                    }
                }
            }
        } else {
            None
        };

        let content_length: usize = res
            .content_length
            .and_then(|length| length.try_into().ok())
            .unwrap_or(0);

        // NOTE: we record the download, even though we don't know if the caller
        // actually consumes the stream.
        // For the current usage, that's fine.
        self.otel_metrics
            .record_download_metrics(content_length as u64, range.as_ref());

        Ok(StreamingBlob {
            path: path.into(),
            mime: res
                .content_type
                .as_ref()
                .unwrap()
                .parse()
                .unwrap_or(mime::APPLICATION_OCTET_STREAM),
            date_updated,
            etag,
            content_length,
            content: Box::new(res.body.into_async_read()),
            compression,
        })
    }

    async fn upload_stream(&self, upload: StreamUpload) -> Result<(), Error> {
        let StreamUpload {
            path,
            mime,
            source,
            compression,
        } = upload;

        match &source {
            StreamUploadSource::Bytes(bytes) => {
                self.upload_stream_single(
                    &path,
                    &mime,
                    &source,
                    compression,
                    bytes.len() as u64,
                    None,
                )
                .await
            }
            StreamUploadSource::File(local_path) => {
                let content_length = fs::metadata(local_path).await?.len();

                if content_length > S3_MULTIPART_UPLOAD_THRESHOLD {
                    self.upload_file_multipart(
                        &path,
                        &mime,
                        local_path,
                        compression,
                        content_length,
                    )
                    .await
                } else {
                    let local_path = local_path.clone();
                    let checksum_crc32 =
                        spawn_blocking(move || Ok(b64.encode(crc32_for_path(local_path)?)))
                            .await?
                            .to_string();

                    self.upload_stream_single(
                        &path,
                        &mime,
                        &source,
                        compression,
                        content_length,
                        Some(&checksum_crc32),
                    )
                    .await
                }
            }
        }
    }

    async fn list_prefix<'a>(&'a self, prefix: &'a str) -> BoxStream<'a, Result<String, Error>> {
        Box::pin(try_stream! {
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
        })
    }

    async fn delete_prefix(&self, prefix: &str) -> Result<(), Error> {
        let stream = self.list_prefix(prefix).await;
        let mut chunks = stream.chunks(900); // 1000 is the limit for the delete_objects API

        while let Some(batch) = chunks.next().await {
            let batch: Vec<_> = batch.into_iter().collect::<anyhow::Result<_>>()?;

            let batch_size = batch.len() as u64;

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
                let successful_deletes = batch_size - errs.len() as u64;
                if successful_deletes > 0 {
                    // we can have partial success, where some of the objects were deleted,
                    // and some not.
                    self.otel_metrics.deleted_files.add(successful_deletes, &[]);
                }

                for err in &errs {
                    error!("error deleting file from s3: {:?}", err);
                }

                anyhow::bail!("deleting from s3 failed");
            }

            self.otel_metrics.deleted_files.add(batch_size, &[]);
        }
        Ok(())
    }
}
