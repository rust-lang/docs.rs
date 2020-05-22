use super::*;
use crate::storage::test::assert_blob_eq;
use rusoto_s3::{
    CreateBucketRequest, DeleteBucketRequest, DeleteObjectRequest, ListObjectsRequest, S3,
};
use std::cell::RefCell;

pub(crate) struct TestS3(RefCell<S3Backend<'static>>);

impl TestS3 {
    pub(crate) fn new() -> Self {
        // A random bucket name is generated and used for the current connection.
        // This allows each test to create a fresh bucket to test with.
        let bucket = format!("docs-rs-test-bucket-{}", rand::random::<u64>());
        let client = s3_client().unwrap();
        let mut runtime = Runtime::new().unwrap();

        runtime
            .block_on(client.create_bucket(CreateBucketRequest {
                bucket: bucket.clone(),
                ..Default::default()
            }))
            .expect("failed to create test bucket");

        let bucket = Box::leak(bucket.into_boxed_str());
        TestS3(RefCell::new(S3Backend::with_runtime(
            client, bucket, runtime,
        )))
    }

    pub(crate) fn upload(&self, blobs: &[Blob]) -> Result<(), Error> {
        self.0.borrow_mut().store_batch(blobs)
    }

    pub(crate) fn assert_404(&self, path: &'static str) {
        use rusoto_s3::GetObjectError;

        let err = self.0.borrow_mut().get(path).unwrap_err();
        match err
            .downcast_ref::<RusotoError<GetObjectError>>()
            .expect("wanted GetObject")
        {
            RusotoError::Unknown(http) => assert_eq!(http.status, 404),
            RusotoError::Service(GetObjectError::NoSuchKey(_)) => {}
            x => panic!("wrong error: {:?}", x),
        };
    }

    pub(crate) fn assert_blob(&self, blob: &Blob, path: &str) {
        let actual = self.0.borrow_mut().get(path).unwrap();
        assert_blob_eq(blob, &actual);
    }
}

impl Drop for TestS3 {
    fn drop(&mut self) {
        // delete the bucket when the test ends
        // this has to delete all the objects in the bucket first or min.io will give an error
        let inner = self.0.borrow();
        let list_req = ListObjectsRequest {
            bucket: inner.bucket.to_owned(),
            ..Default::default()
        };

        let objects = inner
            .runtime
            .handle()
            .block_on(inner.client.list_objects(list_req))
            .unwrap();
        assert!(!objects.is_truncated.unwrap_or(false));

        for path in objects.contents.unwrap() {
            let delete_req = DeleteObjectRequest {
                bucket: inner.bucket.to_owned(),
                key: path.key.unwrap(),
                ..Default::default()
            };

            inner
                .runtime
                .handle()
                .block_on(inner.client.delete_object(delete_req))
                .unwrap();
        }

        let delete_req = DeleteBucketRequest {
            bucket: inner.bucket.to_owned(),
        };

        inner
            .runtime
            .handle()
            .block_on(inner.client.delete_bucket(delete_req))
            .expect("failed to delete test bucket");
    }
}
