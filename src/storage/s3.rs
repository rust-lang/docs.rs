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
    pub(super) fn new(client: &'a S3Client, bucket: &'a str) -> Self {
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

fn parse_timespec(raw: &str) -> Result<Timespec, Error> {
    Ok(time::strptime(raw, "%a, %d %b %Y %H:%M:%S %Z")?.to_timespec())
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
}
