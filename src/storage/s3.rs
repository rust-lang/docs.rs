use super::{Blob, StorageTransaction};
use crate::Config;
use chrono::{DateTime, NaiveDateTime, Utc};
use failure::Error;
use futures_util::{
    future::TryFutureExt,
    stream::{FuturesUnordered, StreamExt},
};
use log::warn;
use rusoto_core::{region::Region, RusotoError};
use rusoto_credential::DefaultCredentialsProvider;
use rusoto_s3::{
    DeleteObjectsRequest, GetObjectError, GetObjectRequest, ListObjectsV2Request, ObjectIdentifier,
    PutObjectRequest, S3Client, S3,
};
use std::{convert::TryInto, io::Write};
use tokio::runtime::Runtime;

pub(super) struct S3Backend {
    client: S3Client,
    runtime: Runtime,
    bucket: String,
    #[cfg(test)]
    temporary: bool,
}

impl S3Backend {
    pub(super) fn new(client: S3Client, config: &Config) -> Result<Self, Error> {
        let runtime = Runtime::new()?;

        // Create the temporary S3 bucket during tests.
        if config.s3_bucket_is_temporary {
            if cfg!(not(test)) {
                panic!("safeguard to prevent creating temporary buckets outside of tests");
            }

            runtime
                .handle()
                .block_on(client.create_bucket(rusoto_s3::CreateBucketRequest {
                    bucket: config.s3_bucket.clone(),
                    ..Default::default()
                }))?;
        }

        Ok(Self {
            client,
            runtime,
            bucket: config.s3_bucket.clone(),
            #[cfg(test)]
            temporary: config.s3_bucket_is_temporary,
        })
    }

    pub(super) fn get(&self, path: &str, max_size: usize) -> Result<Blob, Error> {
        self.runtime.handle().block_on(async {
            let response = self
                .client
                .get_object(GetObjectRequest {
                    bucket: self.bucket.to_string(),
                    key: path.into(),
                    ..Default::default()
                })
                .await;

            let res = match response {
                Ok(res) => res,
                Err(RusotoError::Service(GetObjectError::NoSuchKey(_))) => {
                    return Err(super::PathNotFoundError.into());
                }
                Err(RusotoError::Unknown(http)) if http.status == 404 => {
                    return Err(super::PathNotFoundError.into());
                }
                Err(err) => return Err(err.into()),
            };

            let mut content = crate::utils::sized_buffer::SizedBuffer::new(max_size);
            content.reserve(
                res.content_length
                    .and_then(|l| l.try_into().ok())
                    .unwrap_or(0),
            );

            let mut body = res
                .body
                .ok_or_else(|| failure::err_msg("Received a response from S3 with no body"))?;

            while let Some(data) = body.next().await.transpose()? {
                content.write_all(data.as_ref())?;
            }

            let date_updated = parse_timespec(&res.last_modified.unwrap())?;
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

    pub(super) fn start_storage_transaction(&self) -> Result<S3StorageTransaction, Error> {
        Ok(S3StorageTransaction { s3: self })
    }

    #[cfg(test)]
    pub(super) fn cleanup_after_test(&self) -> Result<(), Error> {
        if !self.temporary {
            return Ok(());
        }

        if cfg!(not(test)) {
            panic!("safeguard to prevent deleting the production bucket");
        }

        let mut transaction = Box::new(self.start_storage_transaction()?);
        transaction.delete_prefix("")?;
        transaction.complete()?;

        self.runtime.handle().block_on(self.client.delete_bucket(
            rusoto_s3::DeleteBucketRequest {
                bucket: self.bucket.clone(),
            },
        ))?;

        Ok(())
    }
}

pub(super) struct S3StorageTransaction<'a> {
    s3: &'a S3Backend,
}

impl<'a> StorageTransaction for S3StorageTransaction<'a> {
    fn store_batch(&mut self, mut batch: Vec<Blob>) -> Result<(), Error> {
        self.s3.runtime.handle().block_on(async {
            // Attempt to upload the batch 3 times
            for _ in 0..3 {
                let mut futures = FuturesUnordered::new();
                for blob in batch.drain(..) {
                    futures.push(
                        self.s3
                            .client
                            .put_object(PutObjectRequest {
                                bucket: self.s3.bucket.to_string(),
                                key: blob.path.clone(),
                                body: Some(blob.content.clone().into()),
                                content_type: Some(blob.mime.clone()),
                                content_encoding: blob
                                    .compression
                                    .as_ref()
                                    .map(|alg| alg.to_string()),
                                ..Default::default()
                            })
                            .map_ok(|_| {
                                crate::web::metrics::UPLOADED_FILES_TOTAL.inc_by(1);
                            })
                            .map_err(|err| {
                                log::error!("Failed to upload blob to S3: {:?}", err);
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
        self.s3.runtime.handle().block_on(async {
            let mut continuation_token = None;
            loop {
                let list = self
                    .s3
                    .client
                    .list_objects_v2(ListObjectsV2Request {
                        bucket: self.s3.bucket.clone(),
                        prefix: Some(prefix.into()),
                        continuation_token,
                        ..ListObjectsV2Request::default()
                    })
                    .await?;

                let to_delete = list
                    .contents
                    .unwrap_or_else(Vec::new)
                    .into_iter()
                    .filter_map(|o| o.key)
                    .map(|key| ObjectIdentifier {
                        key,
                        version_id: None,
                    })
                    .collect::<Vec<_>>();

                let resp = self
                    .s3
                    .client
                    .delete_objects(DeleteObjectsRequest {
                        bucket: self.s3.bucket.clone(),
                        delete: rusoto_s3::Delete {
                            objects: to_delete,
                            quiet: None,
                        },
                        ..DeleteObjectsRequest::default()
                    })
                    .await?;

                if let Some(errs) = resp.errors {
                    for err in &errs {
                        log::error!("error deleting file from s3: {:?}", err);
                    }

                    failure::bail!("uploading to s3 failed");
                }

                continuation_token = list.continuation_token;
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

fn parse_timespec(mut raw: &str) -> Result<DateTime<Utc>, Error> {
    raw = raw.trim_end_matches(" GMT");

    Ok(DateTime::from_utc(
        NaiveDateTime::parse_from_str(raw, "%a, %d %b %Y %H:%M:%S")?,
        Utc,
    ))
}

pub(super) fn s3_client() -> Option<S3Client> {
    // If AWS keys aren't configured, then presume we should use the DB exclusively
    // for file storage.
    if std::env::var_os("AWS_ACCESS_KEY_ID").is_none() && std::env::var_os("FORCE_S3").is_none() {
        return None;
    }

    let creds = match DefaultCredentialsProvider::new() {
        Ok(creds) => creds,
        Err(err) => {
            warn!("failed to retrieve AWS credentials: {}", err);
            return None;
        }
    };

    Some(S3Client::new_with(
        rusoto_core::request::HttpClient::new().unwrap(),
        creds,
        std::env::var("S3_ENDPOINT")
            .ok()
            .map(|e| Region::Custom {
                name: std::env::var("S3_REGION").unwrap_or_else(|_| "us-west-1".to_owned()),
                endpoint: e,
            })
            .unwrap_or(Region::UsWest1),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    #[test]
    fn test_parse_timespec() {
        // Test valid conversions
        assert_eq!(
            parse_timespec("Thu, 1 Jan 1970 00:00:00 GMT").unwrap(),
            Utc.ymd(1970, 1, 1).and_hms(0, 0, 0),
        );
        assert_eq!(
            parse_timespec("Mon, 16 Apr 2018 04:33:50 GMT").unwrap(),
            Utc.ymd(2018, 4, 16).and_hms(4, 33, 50),
        );

        // Test invalid conversion
        assert!(parse_timespec("foo").is_err());
    }

    // The tests for this module are in src/storage/mod.rs, as part of the backend tests. Please
    // add any test checking the public interface there.

    // NOTE: trying to upload a file ending with `/` will behave differently in test and prod.
    // NOTE: On s3, it will succeed and create a file called `/`.
    // NOTE: On min.io, it will fail with 'Object name contains unsupported characters.'
}
