use crate::Config;
use anyhow::{Error, Result};
use aws_sdk_cloudfront::{
    model::{InvalidationBatch, Paths},
    Client, RetryConfig,
};
use std::sync::{Arc, Mutex};
use strum::EnumString;
use tokio::runtime::Runtime;
use uuid::Uuid;

#[derive(Debug, EnumString)]
pub(crate) enum CdnKind {
    #[strum(ascii_case_insensitive)]
    Dummy,

    #[strum(ascii_case_insensitive)]
    CloudFront,
}

pub enum CdnBackend {
    Dummy(Arc<Mutex<Vec<(String, String)>>>),
    CloudFront { runtime: Arc<Runtime> },
}

impl CdnBackend {
    pub fn new(config: &Arc<Config>, runtime: &Arc<Runtime>) -> CdnBackend {
        match config.cdn_backend {
            CdnKind::CloudFront => Self::CloudFront {
                runtime: runtime.clone(),
            },
            CdnKind::Dummy => Self::Dummy(Arc::new(Mutex::new(Vec::new()))),
        }
    }
    /// create a Front invalidation request for a list of path patterns.
    /// patterns can be
    /// * `/filename.ext` (a specific path)
    /// * `/directory-path/file-name.*` (delete these files, all extensions)
    /// * `/directory-path/*` (invalidate all of the files in a directory, without subdirectories)
    /// * `/directory-path*` (recursive directory delete, including subdirectories)
    /// see https://docs.aws.amazon.com/AmazonCloudFront/latest/DeveloperGuide/Invalidation.html#invalidation-specifying-objects
    ///
    /// Returns the caller reference that can be used to query the status of this
    /// invalidation request.
    pub(crate) fn create_invalidation(
        &self,
        distribution_id: &str,
        path_patterns: &[&str],
    ) -> Result<Uuid, Error> {
        let caller_reference = Uuid::new_v4();

        match *self {
            CdnBackend::CloudFront { ref runtime } => {
                let shared_config = runtime.block_on(aws_config::load_from_env());
                let config_builder = aws_sdk_cloudfront::config::Builder::from(&shared_config)
                    .retry_config(RetryConfig::new().with_max_attempts(3));

                runtime.block_on(CdnBackend::cloudfront_invalidation(
                    &Client::from_conf(config_builder.build()),
                    distribution_id,
                    &format!("{}", caller_reference),
                    path_patterns,
                ))?;
            }
            CdnBackend::Dummy(ref invalidation_requests) => {
                let mut invalidation_requests = invalidation_requests
                    .lock()
                    .expect("could not lock mutex on dummy CDN");

                invalidation_requests.extend(
                    path_patterns
                        .iter()
                        .map(|p| (distribution_id.to_owned(), (*p).to_owned())),
                );
            }
        }

        Ok(caller_reference)
    }

    async fn cloudfront_invalidation(
        client: &Client,
        distribution_id: &str,
        caller_reference: &str,
        path_patterns: &[&str],
    ) -> Result<(), Error> {
        let path_patterns: Vec<_> = path_patterns.iter().cloned().map(String::from).collect();

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
                    .caller_reference(caller_reference)
                    .build(),
            )
            .send()
            .await?;

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test::wrapper;

    use aws_sdk_cloudfront::{Client, Config, Credentials, Region};
    use aws_smithy_client::{erase::DynConnector, test_connection::TestConnection};
    use aws_smithy_http::body::SdkBody;

    #[test]
    fn create_cloudfront() {
        wrapper(|env| {
            env.override_config(|config| {
                config.cdn_backend = CdnKind::CloudFront;
            });

            assert!(matches!(*env.cdn(), CdnBackend::CloudFront { .. }));
            assert!(matches!(
                CdnBackend::new(&env.config(), &env.runtime()),
                CdnBackend::CloudFront { .. }
            ));

            Ok(())
        })
    }

    #[test]
    fn create_dummy() {
        wrapper(|env| {
            assert!(matches!(*env.cdn(), CdnBackend::Dummy { .. }));
            assert!(matches!(
                CdnBackend::new(&env.config(), &env.runtime()),
                CdnBackend::Dummy { .. }
            ));

            Ok(())
        })
    }

    async fn get_mock_config() -> aws_sdk_cloudfront::Config {
        let cfg = aws_config::from_env()
            .region(Region::new("eu-central-1"))
            .credentials_provider(Credentials::new(
                "accesskey",
                "privatekey",
                None,
                None,
                "dummy",
            ))
            .load()
            .await;

        Config::new(&cfg)
    }

    #[tokio::test]
    async fn invalidate_path() {
        let conn = TestConnection::new(vec![(
            http::Request::builder()
                .header("content-type", "application/xml")
                .uri(http::uri::Uri::from_static(
                    "https://cloudfront.amazonaws.com/2020-05-31/distribution/some_distribution/invalidation",
                ))
                .body(SdkBody::from(
                    r#"<InvalidationBatch xmlns="http://cloudfront.amazonaws.com/doc/2020-05-31/"><Paths><Quantity>2</Quantity><Items><Path>/some/path*</Path><Path>/another/path/*</Path></Items></Paths><CallerReference>some_reference</CallerReference></InvalidationBatch>"#,
                ))
                .unwrap(),
            http::Response::builder()
                .status(200)
                .body(SdkBody::from(
                    r#"
                    <Invalidation>
                      <CreateTime>2019-12-05T18:40:49.413Z</CreateTime>
                      <Id>I2J0I21PCUYOIK</Id>
                      <InvalidationBatch>
                        <CallerReference>some_reference</CallerReference>
                        <Paths>
                          <Items>
                            <Path>/some/path*</Path>
                            <Path>/another/path/*</Path>
                          </Items>
                          <Quantity>2</Quantity>
                        </Paths>
                      </InvalidationBatch>
                      <Status>InProgress</Status>
                    </Invalidation>
                "#,
                ))
                .unwrap(),
        )]);
        let client =
            Client::from_conf_conn(get_mock_config().await, DynConnector::new(conn.clone()));

        CdnBackend::cloudfront_invalidation(
            &client,
            "some_distribution",
            "some_reference",
            &["/some/path*", "/another/path/*"],
        )
        .await
        .expect("error creating invalidation");

        assert_eq!(conn.requests().len(), 1);
        conn.assert_requests_match(&[]);
    }
}
