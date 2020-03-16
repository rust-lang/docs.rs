use super::Blob;
use failure::Error;
use rusoto_s3::{GetObjectRequest, S3Client, S3};
use std::convert::TryInto;
use std::io::Read;
use time::Timespec;

pub(crate) struct S3Backend<'a> {
    client: &'a S3Client,
    bucket: &'a str,
}

impl<'a> S3Backend<'a> {
    pub(crate) fn new(client: &'a S3Client, bucket: &'a str) -> Self {
        Self {
            client,
            bucket,
        }
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
}

#[cfg(not(test))]
const TIME_FMT: &str = "%a, %d %b %Y %H:%M:%S %Z";
#[cfg(test)]
pub(crate) const TIME_FMT: &str = "%a, %d %b %Y %H:%M:%S %Z";

fn parse_timespec(raw: &str) -> Result<Timespec, Error> {
    Ok(time::strptime(raw, TIME_FMT)?.to_timespec())
}

#[cfg(test)]
mod tests {
    use super::*;

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
            let s3 = env.s3_upload(blob.clone(), "<test bucket>");

            let backend = S3Backend::new(&s3.client, &s3.bucket);

            // Test that the proper file was returned
            assert_eq!(blob, backend.get("dir/foo.txt")?);

            /*
            // Test that other files are not returned
            assert!(backend
                .get("dir/bar.txt")
                .unwrap_err()
                .downcast_ref::<PathNotFoundError>()
                .is_some());
            assert!(backend
                .get("foo.txt")
                .unwrap_err()
                .downcast_ref::<PathNotFoundError>()
                .is_some());
            */

            Ok(())
        });
    }
}
