use super::Blob;
use failure::Error;
use futures::Future;
use log::{error, warn};
use rusoto_core::region::Region;
use rusoto_credential::DefaultCredentialsProvider;
use rusoto_s3::{GetObjectRequest, PutObjectRequest, S3Client, S3};
use std::convert::TryInto;
use std::io::Read;
use time::Timespec;

pub(crate) struct S3Backend<'a> {
    client: S3Client,
    bucket: &'a str,
}

impl<'a> S3Backend<'a> {
    pub(crate) fn new(client: S3Client, bucket: &'a str) -> Self {
        Self { client, bucket }
    }

    pub(super) fn get(&self, path: &str) -> Result<Blob, Error> {
        let res = self
            .client
            .get_object(GetObjectRequest {
                bucket: self.bucket.to_string(),
                key: path.into(),
                ..Default::default()
            })
            .sync()?;

        let mut b = res.body.unwrap().into_blocking_read();
        let mut content = Vec::with_capacity(
            res.content_length
                .and_then(|l| l.try_into().ok())
                .unwrap_or(0),
        );
        b.read_to_end(&mut content).unwrap();

        let date_updated = parse_timespec(&res.last_modified.unwrap())?;

        Ok(Blob {
            path: path.into(),
            mime: res.content_type.unwrap(),
            date_updated,
            content,
        })
    }

    pub(super) fn store_batch(&self, batch: &[Blob]) -> Result<(), Error> {
        let mut rt = tokio::runtime::Runtime::new().unwrap();
        let mut attempts = 0;

        loop {
            let mut futures = Vec::with_capacity(batch.len());
            for blob in batch {
                futures.push(
                    self.client
                        .put_object(PutObjectRequest {
                            bucket: self.bucket.to_string(),
                            key: blob.path.clone(),
                            body: Some(blob.content.clone().into()),
                            content_type: Some(blob.mime.clone()),
                            ..Default::default()
                        })
                        .inspect(|_| {
                            crate::web::metrics::UPLOADED_FILES_TOTAL.inc_by(1);
                        }),
                );
            }
            attempts += 1;

            match rt.block_on(::futures::future::join_all(futures)) {
                // this batch was successful, start another batch if there are still more files
                Ok(_) => break,
                Err(err) => {
                    error!("failed to upload to s3: {:?}", err);
                    // if a futures error occurs, retry the batch
                    if attempts > 2 {
                        panic!("failed to upload 3 times, exiting");
                    }
                }
            }
        }
        Ok(())
    }
}

// public for testing
pub(crate) const TIME_FMT: &str = "%a, %d %b %Y %H:%M:%S %Z";

fn parse_timespec(raw: &str) -> Result<Timespec, Error> {
    Ok(time::strptime(raw, TIME_FMT)?.to_timespec())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test::TestEnvironment;

    fn assert_s3_404(env: &TestEnvironment, path: &'static str) {
        use rusoto_core::RusotoError;
        use rusoto_s3::GetObjectError;

        let s3 = env.s3().not_found(path);
        let backend = S3Backend::new(s3.client, s3.bucket);
        let err = backend.get(path).unwrap_err();
        let status = match err
            .downcast_ref::<RusotoError<GetObjectError>>()
            .expect("wanted GetObject")
        {
            RusotoError::Unknown(http) => http.status,
            _ => panic!("wrong error"),
        };
        assert_eq!(status, 404);
    }

    #[test]
    fn test_parse_timespec() {
        crate::test::wrapper(|_| {
            // Test valid conversions
            assert_eq!(
                parse_timespec("Thu, 1 Jan 1970 00:00:00 GMT")?,
                Timespec::new(0, 0)
            );
            assert_eq!(
                parse_timespec("Mon, 16 Apr 2018 04:33:50 GMT")?,
                Timespec::new(1523853230, 0)
            );

            // Test invalid conversion
            assert!(parse_timespec("foo").is_err());

            Ok(())
        })
    }

    #[test]
    fn test_path_get() {
        crate::test::wrapper(|env| {
            let blob = Blob {
                path: "dir/foo.txt".into(),
                mime: "text/plain".into(),
                date_updated: Timespec::new(42, 0),
                content: "Hello world!".into(),
            };

            // Add a test file to the database
            let s3 = env.s3().upload(blob.clone());

            let backend = S3Backend::new(s3.client, &s3.bucket);

            // Test that the proper file was returned
            assert_eq!(blob, backend.get("dir/foo.txt")?);

            // Test that other files are not returned
            assert_s3_404(&env, "dir/bar.txt");
            assert_s3_404(&env, "foo.txt");

            Ok(())
        });
    }
}

pub(crate) static S3_BUCKET_NAME: &str = "rust-docs-rs";

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
