use anyhow::Error;
use aws_sdk_cloudfront::{
    model::{InvalidationBatch, Paths},
    Client, RetryConfig,
};
use tokio::runtime::Runtime;
use uuid::Uuid;

/// create a CloudFront invalidation request for a list of path patterns.
/// patterns can be
/// * `/filename.ext` (a specific path)
/// * `/directory-path/file-name.*` (delete these files, all extensions)
/// * `/directory-path/*` (invalidate all of the files in a directory, without subdirectories)
/// * `/directory-path*` (recursive directory delete, including subdirectories)
/// see https://docs.aws.amazon.com/AmazonCloudFront/latest/DeveloperGuide/Invalidation.html#invalidation-specifying-objects
///
/// Returns the caller reference that can be used to query the status of this
/// invalidation request.
pub(crate) fn create_cloudfront_invalidation(
    runtime: &Runtime,
    distribution_id: &str,
    path_patterns: &[&str],
) -> Result<Uuid, Error> {
    let shared_config = runtime.block_on(aws_config::load_from_env());
    let config_builder = aws_sdk_cloudfront::config::Builder::from(&shared_config)
        .retry_config(RetryConfig::new().with_max_attempts(3));

    let client = Client::from_conf(config_builder.build());
    let path_patterns: Vec<_> = path_patterns.iter().cloned().map(String::from).collect();

    let caller_reference = Uuid::new_v4();

    runtime.block_on(async {
        client
            .create_invalidation()
            .distribution_id(distribution_id)
            .invalidation_batch(
                InvalidationBatch::builder()
                    .paths(
                        Paths::builder()
                            .quantity(path_patterns.len().try_into().unwrap())
                            .set_items(Some(path_patterns))
                            .build(),
                    )
                    .caller_reference(format!("{}", caller_reference))
                    .build(),
            )
            .send()
            .await
    })?;

    Ok(caller_reference)
}
