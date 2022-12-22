use crate::Config;
use anyhow::{Context, Error, Result};
use aws_sdk_cloudfront::{
    config::retry::RetryConfig,
    model::{InvalidationBatch, Paths},
    Client, Region,
};
use chrono::{DateTime, Utc};
use serde::Serialize;
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

#[derive(Debug)]
pub enum CdnBackend {
    Dummy(Arc<Mutex<Vec<(String, String)>>>),
    CloudFront {
        runtime: Arc<Runtime>,
        client: Client,
    },
}

impl CdnBackend {
    pub fn new(config: &Arc<Config>, runtime: &Arc<Runtime>) -> CdnBackend {
        match config.cdn_backend {
            CdnKind::CloudFront => {
                let shared_config = runtime.block_on(aws_config::load_from_env());
                let config_builder = aws_sdk_cloudfront::config::Builder::from(&shared_config)
                    .retry_config(
                        RetryConfig::standard().with_max_attempts(config.aws_sdk_max_retries),
                    )
                    .region(Region::new(config.s3_region.clone()));

                Self::CloudFront {
                    runtime: runtime.clone(),
                    client: Client::from_conf(config_builder.build()),
                }
            }
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
            CdnBackend::CloudFront {
                ref runtime,
                ref client,
            } => {
                runtime.block_on(CdnBackend::cloudfront_invalidation(
                    client,
                    distribution_id,
                    &caller_reference.to_string(),
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

pub(crate) fn invalidate_crate(config: &Config, cdn: &CdnBackend, name: &str) -> Result<()> {
    if let Some(distribution_id) = config.cloudfront_distribution_id_web.as_ref() {
        cdn.create_invalidation(
            distribution_id,
            &[&format!("/{}*", name), &format!("/crate/{}*", name)],
        )
        .context("error creating web CDN invalidation")?;
    }
    if let Some(distribution_id) = config.cloudfront_distribution_id_static.as_ref() {
        cdn.create_invalidation(distribution_id, &[&format!("/rustdoc/{}*", name)])
            .context("error creating static CDN invalidation")?;
    }

    Ok(())
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub(crate) struct CrateInvalidation {
    pub name: String,
    pub created: DateTime<Utc>,
}

/// Return fake active cloudfront invalidations.
/// CloudFront invalidations can take up to 15 minutes. Until we have
/// live queries of the invalidation status we just assume it's fine
/// 20 minutes after the build.
/// TODO: should be replaced be keeping track or querying the active invalidation from CloudFront
pub(crate) fn active_crate_invalidations(
    conn: &mut postgres::Client,
) -> Result<Vec<CrateInvalidation>> {
    Ok(conn
        .query(
            r#"
             SELECT
                 crates.name,
                 MIN(builds.build_time) as build_time
             FROM crates
             INNER JOIN releases ON crates.id = releases.crate_id
             INNER JOIN builds ON releases.id = builds.rid
             WHERE builds.build_time >= CURRENT_TIMESTAMP - INTERVAL '20 minutes'
             GROUP BY crates.name
             ORDER BY MIN(builds.build_time)"#,
            &[],
        )?
        .iter()
        .map(|row| CrateInvalidation {
            name: row.get(0),
            created: row.get(1),
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test::{wrapper, FakeBuild};

    use aws_sdk_cloudfront::{Client, Config, Credentials, Region};
    use aws_smithy_client::{
        erase::DynConnector, http_connector::HttpConnector, test_connection::TestConnection,
    };
    use aws_smithy_http::body::SdkBody;
    use chrono::{Duration, Timelike};

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

    #[test]
    fn invalidate_a_crate() {
        crate::test::wrapper(|env| {
            env.override_config(|config| {
                config.cloudfront_distribution_id_web = Some("distribution_id_web".into());
                config.cloudfront_distribution_id_static = Some("distribution_id_static".into());
            });
            invalidate_crate(&env.config(), &env.cdn(), "krate")?;

            assert!(matches!(*env.cdn(), CdnBackend::Dummy(_)));
            if let CdnBackend::Dummy(ref invalidation_requests) = *env.cdn() {
                let ir = invalidation_requests.lock().unwrap();
                assert_eq!(
                    *ir,
                    [
                        ("distribution_id_web".into(), "/krate*".into()),
                        ("distribution_id_web".into(), "/crate/krate*".into()),
                        ("distribution_id_static".into(), "/rustdoc/krate*".into()),
                    ]
                );
            }
            Ok(())
        });
    }

    async fn get_mock_config(
        http_connector: impl Into<HttpConnector>,
    ) -> aws_sdk_cloudfront::Config {
        let cfg = aws_config::from_env()
            .region(Region::new("eu-central-1"))
            .credentials_provider(Credentials::new(
                "accesskey",
                "privatekey",
                None,
                None,
                "dummy",
            ))
            .http_connector(http_connector)
            .load()
            .await;

        Config::new(&cfg)
    }

    #[test]
    fn get_active_invalidations() {
        wrapper(|env| {
            let now = Utc::now().with_nanosecond(0).unwrap();
            let past_deploy = now - Duration::minutes(21);
            let first_running_deploy = now - Duration::minutes(10);
            let second_running_deploy = now;

            env.fake_release()
                .name("krate_2")
                .version("0.0.1")
                .builds(vec![FakeBuild::default().build_time(first_running_deploy)])
                .create()?;

            env.fake_release()
                .name("krate_2")
                .version("0.0.2")
                .builds(vec![FakeBuild::default().build_time(second_running_deploy)])
                .create()?;

            env.fake_release()
                .name("krate_1")
                .version("0.0.2")
                .builds(vec![FakeBuild::default().build_time(second_running_deploy)])
                .create()?;

            env.fake_release()
                .name("krate_1")
                .version("0.0.3")
                .builds(vec![FakeBuild::default().build_time(past_deploy)])
                .create()?;

            assert_eq!(
                active_crate_invalidations(&mut env.db().conn())?,
                vec![
                    CrateInvalidation {
                        name: "krate_2".into(),
                        created: first_running_deploy,
                    },
                    CrateInvalidation {
                        name: "krate_1".into(),
                        created: second_running_deploy,
                    }
                ]
            );

            Ok(())
        })
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
        let client = Client::from_conf(get_mock_config(DynConnector::new(conn.clone())).await);

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
