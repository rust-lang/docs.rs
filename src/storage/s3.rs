use super::{Blob, StorageTransaction};
use crate::Config;
use chrono::{DateTime, NaiveDateTime, Utc};
use failure::Error;
use futures_util::{
    future::TryFutureExt,
    stream::{FuturesUnordered, StreamExt},
};
use log::warn;
use once_cell::sync::Lazy;
use rusoto_core::{region::Region, RusotoError};
use rusoto_credential::DefaultCredentialsProvider;
use rusoto_s3::{GetObjectError, GetObjectRequest, PutObjectRequest, S3Client, S3};
use std::{convert::TryInto, io::Write};
use tokio::runtime::Runtime;

pub(crate) static S3_RUNTIME: Lazy<Runtime> =
    Lazy::new(|| Runtime::new().expect("Failed to create S3 runtime"));

pub(crate) struct S3Backend {
    pub client: S3Client,
    bucket: String,
    #[cfg(test)]
    temporary: bool,
}

impl S3Backend {
    pub(crate) fn new(client: S3Client, config: &Config) -> Result<Self, Error> {
        // Create the temporary S3 bucket during tests.
        if config.s3_bucket_is_temporary {
            if cfg!(not(test)) {
                panic!("safeguard to prevent creating temporary buckets outside of tests");
            }

            S3_RUNTIME
                .handle()
                .block_on(client.create_bucket(rusoto_s3::CreateBucketRequest {
                    bucket: config.s3_bucket.clone(),
                    ..Default::default()
                }))?;
        }

        Ok(Self {
            client,
            bucket: config.s3_bucket.clone(),
            #[cfg(test)]
            temporary: config.s3_bucket_is_temporary,
        })
    }

    pub(super) fn get(&self, path: &str, max_size: usize) -> Result<Blob, Error> {
        S3_RUNTIME.handle().block_on(async {
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

        // TODO: the following code was copy/pasted from the old TestS3, it will be replaced with
        // better, more resilient and tested code in a later commit.

        // delete the bucket when the test ends
        // this has to delete all the objects in the bucket first or min.io will give an error
        S3_RUNTIME.handle().block_on(async {
            let list_req = rusoto_s3::ListObjectsRequest {
                bucket: self.bucket.to_owned(),
                ..Default::default()
            };
            let objects = self.client.list_objects(list_req).await?;
            assert!(!objects.is_truncated.unwrap_or(false));
            for path in objects.contents.unwrap_or_else(Vec::new) {
                let delete_req = rusoto_s3::DeleteObjectRequest {
                    bucket: self.bucket.clone(),
                    key: path.key.unwrap(),
                    ..Default::default()
                };
                self.client.delete_object(delete_req).await?;
            }
            let delete_req = rusoto_s3::DeleteBucketRequest {
                bucket: self.bucket.clone(),
            };
            self.client.delete_bucket(delete_req).await?;

            Ok::<(), Error>(())
        })?;

        Ok(())
    }
}

pub(super) struct S3StorageTransaction<'a> {
    s3: &'a S3Backend,
}

impl<'a> StorageTransaction for S3StorageTransaction<'a> {
    fn store_batch(&mut self, mut batch: Vec<Blob>) -> Result<(), Error> {
        S3_RUNTIME.handle().block_on(async {
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

pub(crate) fn s3_client() -> Option<S3Client> {
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
pub(crate) mod tests {
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
