use crate::{metrics::duration_to_seconds, utils::report_error, Config, Metrics};
use anyhow::{anyhow, Context, Error, Result};
use aws_sdk_cloudfront::{
    config::retry::RetryConfig,
    model::{InvalidationBatch, Paths},
    Client, Region,
};
use chrono::{DateTime, Utc};
use futures_util::StreamExt;
use serde::Serialize;
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};
use strum::EnumString;
use tokio::runtime::Runtime;
use tracing::{debug, info, instrument, warn};
use uuid::Uuid;

/// maximum amout of parallel in-progress wildcard invalidations
/// The actual limit is 15, but we want to keep some room for manually
/// triggered invalidations
const MAX_CLOUDFRONT_WILDCARD_INVALIDATIONS: i32 = 13;

#[derive(Debug, EnumString)]
pub(crate) enum CdnKind {
    #[strum(ascii_case_insensitive)]
    Dummy,

    #[strum(ascii_case_insensitive)]
    CloudFront,
}

#[derive(Debug, PartialEq, Eq, Clone)]
pub struct CdnInvalidation {
    pub(crate) distribution_id: String,
    pub(crate) invalidation_id: String,
    pub(crate) path_patterns: Vec<String>,
}

#[derive(Debug)]
pub enum CdnBackend {
    Dummy {
        invalidation_requests: Arc<Mutex<Vec<CdnInvalidation>>>,
    },
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
            CdnKind::Dummy => Self::Dummy {
                invalidation_requests: Arc::new(Mutex::new(Vec::new())),
            },
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
    #[instrument]
    fn create_invalidation(
        &self,
        distribution_id: &str,
        path_patterns: &[&str],
    ) -> Result<CdnInvalidation, Error> {
        let caller_reference = Uuid::new_v4();

        match *self {
            CdnBackend::CloudFront {
                ref runtime,
                ref client,
                ..
            } => {
                let id = runtime.block_on(CdnBackend::create_cloudfront_invalidation(
                    client,
                    distribution_id,
                    &caller_reference.to_string(),
                    path_patterns,
                ))?;
                Ok(CdnInvalidation {
                    distribution_id: distribution_id.to_owned(),
                    invalidation_id: id,
                    path_patterns: path_patterns.iter().cloned().map(str::to_owned).collect(),
                })
            }
            CdnBackend::Dummy {
                ref invalidation_requests,
                ..
            } => {
                let mut invalidation_requests = invalidation_requests
                    .lock()
                    .expect("could not lock mutex on dummy CDN");

                let invalidation = CdnInvalidation {
                    distribution_id: distribution_id.to_owned(),
                    invalidation_id: caller_reference.to_string(),
                    path_patterns: path_patterns.iter().cloned().map(str::to_owned).collect(),
                };

                invalidation_requests.push(invalidation.clone());
                Ok(invalidation)
            }
        }
    }

    #[cfg(test)]
    fn clear_active_invalidations(&self) {
        match self {
            CdnBackend::Dummy {
                invalidation_requests,
                ..
            } => {
                invalidation_requests
                    .lock()
                    .expect("could not lock mutex on dummy CDN")
                    .clear();
            }
            CdnBackend::CloudFront { .. } => unreachable!(),
        }
    }

    fn active_invalidations(&self, distribution_id: &str) -> Result<Vec<CdnInvalidation>, Error> {
        match self {
            CdnBackend::Dummy {
                invalidation_requests,
                ..
            } => {
                let invalidation_requests = invalidation_requests
                    .lock()
                    .expect("could not lock mutex on dummy CDN");

                Ok(invalidation_requests
                    .iter()
                    .filter(|i| i.distribution_id == distribution_id)
                    .cloned()
                    .collect())
            }
            CdnBackend::CloudFront {
                runtime, client, ..
            } => Ok(
                runtime.block_on(CdnBackend::list_active_cloudfront_invalidations(
                    client,
                    distribution_id,
                ))?,
            ),
        }
    }

    #[instrument]
    async fn list_active_cloudfront_invalidations(
        client: &Client,
        distribution_id: &str,
    ) -> Result<Vec<CdnInvalidation>, Error> {
        let mut stream = client
            .list_invalidations()
            .distribution_id(distribution_id)
            .into_paginator()
            .items()
            .send();

        let mut ids: Vec<String> = Vec::new();
        while let Some(invalidation) = stream.next().await {
            let invalidation = invalidation?;

            if let (Some(id), Some(status)) = (invalidation.id(), invalidation.status()) {
                // the `ListInvalidation` AWS API doesn't support filtering, so we
                // have to query all invalidations and filter here.
                if status == "InProgress" {
                    ids.push(id.to_owned());
                } else if status != "Completed" {
                    report_error(&anyhow!(
                        "got unknown cloudfront invalidation status: {} in {:?}",
                        status,
                        invalidation
                    ));
                }
            }
        }

        let mut result = Vec::new();
        for id in ids {
            let response = client
                .get_invalidation()
                .distribution_id(distribution_id)
                .id(id.clone())
                .send()
                .await?;

            let mut patterns = Vec::new();
            if let Some(invalidation) = response.invalidation() {
                if let Some(batch) = invalidation.invalidation_batch() {
                    if let Some(paths) = batch.paths() {
                        if let Some(items) = paths.items() {
                            patterns.extend_from_slice(items);
                        }
                    }
                }
            }
            if patterns.is_empty() {
                warn!(
                    id,
                    ?response,
                    "got invalidation detail response without paths"
                );
            }

            result.push(CdnInvalidation {
                distribution_id: distribution_id.to_owned(),
                invalidation_id: id,
                path_patterns: patterns,
            });
        }

        Ok(result)
    }

    #[instrument]
    async fn create_cloudfront_invalidation(
        client: &Client,
        distribution_id: &str,
        caller_reference: &str,
        path_patterns: &[&str],
    ) -> Result<String, Error> {
        let path_patterns: Vec<_> = path_patterns.iter().cloned().map(String::from).collect();

        Ok(client
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
            .await?
            .invalidation()
            .ok_or_else(|| {
                anyhow!("missing invalidation information in create-invalidation result")
            })?
            .id()
            .ok_or_else(|| anyhow!("missing invalidation ID in create-invalidation result"))?
            .to_owned())
    }
}

#[instrument(skip(conn))]
pub(crate) fn handle_queued_invalidation_requests(
    cdn: &CdnBackend,
    metrics: &Metrics,
    conn: &mut impl postgres::GenericClient,
    distribution_id: &str,
) -> Result<()> {
    info!("handling queued CDN invalidations");

    let active_invalidations = cdn
        .active_invalidations(distribution_id)
        .context("error querying active invalidations")?;

    // for now we assume all invalidation paths are wildcard invalidations,
    // so we apply the wildcard limit.
    let active_path_invalidations: usize = active_invalidations
        .iter()
        .map(|i| i.path_patterns.len())
        .sum();

    debug!(
        active_invalidations = active_invalidations.len(),
        active_path_invalidations, "found active invalidations",
    );

    // remove the invalidation from the queue when they are completed.
    // We're only looking at InProgress invalidations,
    // we don't differentiate between `Completed` ones, and invalidations
    // missing in the CloudFront `ListInvalidations` response.
    let now = Utc::now();
    for row in conn.query(
        "DELETE FROM cdn_invalidation_queue
         WHERE 
             cdn_distribution_id = $1 AND 
             created_in_cdn IS NOT NULL AND 
             NOT (cdn_reference = ANY($2))
         RETURNING created_in_cdn
        ",
        &[
            &distribution_id,
            &active_invalidations
                .iter()
                .map(|i| i.invalidation_id.clone())
                .collect::<Vec<_>>(),
        ],
    )? {
        if let Ok(duration) = (now - row.get::<_, DateTime<Utc>>(0)).to_std() {
            // This can only fail when the duration is negative, which can't happen anyways
            metrics
                .cdn_invalidation_time
                .with_label_values(&[distribution_id])
                .observe(duration_to_seconds(duration));
        }
    }

    let possible_path_invalidations: i32 =
        MAX_CLOUDFRONT_WILDCARD_INVALIDATIONS - active_path_invalidations as i32;

    if possible_path_invalidations <= 0 {
        info!(
            active_path_invalidations,
            "too many active cloudfront wildcard invalidations \
            will not create a new one."
        );
        return Ok(());
    }

    // create new an invalidation for the queued path patterns
    let mut transaction = conn.transaction()?;
    let mut path_patterns: Vec<String> = Vec::new();
    let mut queued_entry_ids: Vec<i64> = Vec::new();

    for row in transaction.query(
        "SELECT id, path_pattern, queued
         FROM cdn_invalidation_queue 
         WHERE cdn_distribution_id = $1 AND created_in_cdn IS NULL
         ORDER BY queued, id
         LIMIT $2
         FOR UPDATE",
        &[&distribution_id, &(possible_path_invalidations as i64)],
    )? {
        queued_entry_ids.push(row.get("id"));
        path_patterns.push(row.get("path_pattern"));

        if let Ok(duration) = (now - row.get::<_, DateTime<Utc>>("queued")).to_std() {
            // This can only fail when the duration is negative, which can't happen anyways
            metrics
                .cdn_queue_time
                .with_label_values(&[distribution_id])
                .observe(duration_to_seconds(duration));
        }
    }

    if path_patterns.is_empty() {
        info!("no queued path patterns to invalidate, going back to sleep");
        return Ok(());
    }

    match cdn
        .create_invalidation(
            distribution_id,
            &path_patterns.iter().map(String::as_str).collect::<Vec<_>>(),
        )
        .context("error creating new invalidation")
    {
        Ok(invalidation) => {
            transaction.execute(
                "UPDATE cdn_invalidation_queue 
                 SET 
                     created_in_cdn = CURRENT_TIMESTAMP,
                     cdn_reference = $1
                 WHERE 
                     id = ANY($2)",
                &[&invalidation.invalidation_id, &queued_entry_ids],
            )?;
            transaction.commit()?;
        }
        Err(err) => return Err(err),
    }

    Ok(())
}

#[instrument(skip(conn, config))]
pub(crate) fn queue_crate_invalidation(
    conn: &mut impl postgres::GenericClient,
    config: &Config,
    name: &str,
) -> Result<()> {
    if !config.cache_invalidatable_responses {
        info!("full page cache disabled, skipping queueing invalidation");
        return Ok(());
    }

    let mut add = |distribution_id: &str, path_patterns: &[&str]| -> Result<()> {
        for pattern in path_patterns {
            debug!(distribution_id, pattern, "enqueueing web CDN invalidation");
            conn.execute(
                "INSERT INTO cdn_invalidation_queue (crate, cdn_distribution_id, path_pattern)
                 VALUES ($1, $2, $3)",
                &[&name, &distribution_id, pattern],
            )?;
        }
        Ok(())
    };
    if let Some(distribution_id) = config.cloudfront_distribution_id_web.as_ref() {
        add(
            distribution_id,
            &[&format!("/{name}*"), &format!("/crate/{name}*")],
        )
        .context("error enqueueing web CDN invalidation")?;
    }
    if let Some(distribution_id) = config.cloudfront_distribution_id_static.as_ref() {
        add(distribution_id, &[&format!("/rustdoc/{name}*")])
            .context("error enqueueing static CDN invalidation")?;
    }

    Ok(())
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq, Default)]
pub(crate) struct QueuedInvalidation {
    pub krate: String,
    pub cdn_distribution_id: String,
    pub path_pattern: String,
    pub queued: DateTime<Utc>,
    pub created_in_cdn: Option<DateTime<Utc>>,
    pub cdn_reference: Option<String>,
}

/// Return which crates have queued or active cloudfront invalidations.
pub(crate) fn queued_or_active_crate_invalidations(
    conn: &mut impl postgres::GenericClient,
) -> Result<Vec<QueuedInvalidation>> {
    Ok(conn
        .query(
            r#"
             SELECT
                crate, 
                cdn_distribution_id, 
                path_pattern, 
                queued,
                created_in_cdn,
                cdn_reference
             FROM cdn_invalidation_queue 
             ORDER BY queued, id"#,
            &[],
        )?
        .iter()
        .map(|row| QueuedInvalidation {
            krate: row.get("crate"),
            cdn_distribution_id: row.get("cdn_distribution_id"),
            path_pattern: row.get("path_pattern"),
            queued: row.get("queued"),
            created_in_cdn: row.get("created_in_cdn"),
            cdn_reference: row.get("cdn_reference"),
        })
        .collect())
}

/// Return the count of queued or active invalidations, per distribution id
pub(crate) fn queued_or_active_crate_invalidation_count_by_distribution(
    conn: &mut impl postgres::GenericClient,
    config: &Config,
) -> Result<HashMap<String, i64>> {
    let mut result: HashMap<String, i64> = HashMap::from_iter(
        config
            .cloudfront_distribution_id_web
            .iter()
            .chain(config.cloudfront_distribution_id_static.iter())
            .cloned()
            .map(|id| (id, 0)),
    );

    result.extend(
        conn.query(
            r#"
             SELECT
                cdn_distribution_id, 
                count(*)
             FROM cdn_invalidation_queue 
             GROUP BY cdn_distribution_id"#,
            &[],
        )?
        .iter()
        .map(|row| (row.get(0), row.get(1))),
    );

    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test::wrapper;

    use aws_sdk_cloudfront::{Client, Config, Credentials, Region};
    use aws_smithy_client::{
        erase::DynConnector, http_connector::HttpConnector, test_connection::TestConnection,
    };
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

    #[test]
    fn invalidation_counts_are_zero_with_empty_queue() {
        crate::test::wrapper(|env| {
            env.override_config(|config| {
                config.cloudfront_distribution_id_web = Some("distribution_id_web".into());
                config.cloudfront_distribution_id_static = Some("distribution_id_static".into());
            });

            let config = env.config();
            let mut conn = env.db().conn();
            assert!(queued_or_active_crate_invalidations(&mut *conn)?.is_empty());

            let counts =
                queued_or_active_crate_invalidation_count_by_distribution(&mut *conn, &config)?;
            assert_eq!(counts.len(), 2);
            assert_eq!(*counts.get("distribution_id_web").unwrap(), 0);
            assert_eq!(*counts.get("distribution_id_static").unwrap(), 0);
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

            let cdn = env.cdn();
            let config = env.config();
            let mut conn = env.db().conn();
            assert!(queued_or_active_crate_invalidations(&mut *conn)?.is_empty());

            queue_crate_invalidation(&mut *conn, &env.config(), "krate")?;

            // invalidation paths are queued.
            assert_eq!(
                queued_or_active_crate_invalidations(&mut *conn)?
                    .into_iter()
                    .map(|i| (
                        i.cdn_distribution_id,
                        i.krate,
                        i.path_pattern,
                        i.cdn_reference
                    ))
                    .collect::<Vec<_>>(),
                vec![
                    (
                        "distribution_id_web".into(),
                        "krate".into(),
                        "/krate*".into(),
                        None
                    ),
                    (
                        "distribution_id_web".into(),
                        "krate".into(),
                        "/crate/krate*".into(),
                        None
                    ),
                    (
                        "distribution_id_static".into(),
                        "krate".into(),
                        "/rustdoc/krate*".into(),
                        None
                    ),
                ]
            );

            let counts =
                queued_or_active_crate_invalidation_count_by_distribution(&mut *conn, &config)?;
            assert_eq!(counts.len(), 2);
            assert_eq!(*counts.get("distribution_id_web").unwrap(), 2);
            assert_eq!(*counts.get("distribution_id_static").unwrap(), 1);

            // queueing the invalidation doesn't create it in the CDN
            assert!(cdn.active_invalidations("distribution_id_web")?.is_empty());
            assert!(cdn
                .active_invalidations("distribution_id_static")?
                .is_empty());

            // now handle the queued invalidations
            handle_queued_invalidation_requests(
                &env.cdn(),
                &env.metrics(),
                &mut *conn,
                "distribution_id_web",
            )?;
            handle_queued_invalidation_requests(
                &env.cdn(),
                &env.metrics(),
                &mut *conn,
                "distribution_id_static",
            )?;

            // which creates them in the CDN
            {
                let ir_web = cdn.active_invalidations("distribution_id_web")?;
                assert_eq!(ir_web.len(), 1);
                assert_eq!(ir_web[0].path_patterns, vec!["/krate*", "/crate/krate*"]);

                let ir_static = cdn.active_invalidations("distribution_id_static")?;
                assert_eq!(ir_web.len(), 1);
                assert_eq!(ir_static[0].path_patterns, vec!["/rustdoc/krate*"]);
            }

            // the queued entries got a CDN reference attached
            assert!(queued_or_active_crate_invalidations(&mut *conn)?
                .iter()
                .all(|i| i.cdn_reference.is_some() && i.created_in_cdn.is_some()));

            // clear the active invalidations in the CDN to _fake_ them
            // being completed on the CDN side.
            cdn.clear_active_invalidations();

            // now handle again
            handle_queued_invalidation_requests(
                &env.cdn(),
                &env.metrics(),
                &mut *conn,
                "distribution_id_web",
            )?;
            handle_queued_invalidation_requests(
                &env.cdn(),
                &env.metrics(),
                &mut *conn,
                "distribution_id_static",
            )?;

            // which removes them from the queue table
            assert!(queued_or_active_crate_invalidations(&mut *conn)?.is_empty());

            Ok(())
        });
    }

    #[test]
    fn only_add_some_invalidations_when_too_many_are_active() {
        crate::test::wrapper(|env| {
            env.override_config(|config| {
                config.cloudfront_distribution_id_web = Some("distribution_id_web".into());
            });

            let cdn = env.cdn();

            // create an invalidation with 15 paths, so we're over the limit
            cdn.create_invalidation(
                "distribution_id_web",
                &(0..(MAX_CLOUDFRONT_WILDCARD_INVALIDATIONS - 1))
                    .map(|_| "/something*")
                    .collect::<Vec<_>>(),
            )?;

            let mut conn = env.db().conn();
            assert!(queued_or_active_crate_invalidations(&mut *conn)?.is_empty());

            // queue an invalidation
            queue_crate_invalidation(&mut *conn, &env.config(), "krate")?;

            // handle the queued invalidations
            handle_queued_invalidation_requests(
                &env.cdn(),
                &env.metrics(),
                &mut *conn,
                "distribution_id_web",
            )?;

            // only one path was added to the CDN
            let q = queued_or_active_crate_invalidations(&mut *conn)?;
            assert_eq!(q.iter().filter(|i| i.cdn_reference.is_some()).count(), 1);

            // old invalidation is still active, new one is added
            let ir_web = cdn.active_invalidations("distribution_id_web")?;
            assert_eq!(ir_web.len(), 2);
            assert_eq!(ir_web[0].path_patterns.len(), 12);
            assert_eq!(ir_web[1].path_patterns.len(), 1);

            Ok(())
        });
    }

    #[test]
    fn dont_create_invalidations_when_too_many_are_active() {
        crate::test::wrapper(|env| {
            env.override_config(|config| {
                config.cloudfront_distribution_id_web = Some("distribution_id_web".into());
            });

            let cdn = env.cdn();

            // create an invalidation with 15 paths, so we're over the limit
            cdn.create_invalidation(
                "distribution_id_web",
                &(0..15).map(|_| "/something*").collect::<Vec<_>>(),
            )?;

            let mut conn = env.db().conn();
            assert!(queued_or_active_crate_invalidations(&mut *conn)?.is_empty());

            // queue an invalidation
            queue_crate_invalidation(&mut *conn, &env.config(), "krate")?;

            // handle the queued invalidations
            handle_queued_invalidation_requests(
                &env.cdn(),
                &env.metrics(),
                &mut *conn,
                "distribution_id_web",
            )?;

            // nothing was added to the CDN
            assert!(queued_or_active_crate_invalidations(&mut *conn)?
                .iter()
                .all(|i| i.cdn_reference.is_none()));

            // old invalidations are still active
            let ir_web = cdn.active_invalidations("distribution_id_web")?;
            assert_eq!(ir_web.len(), 1);
            assert_eq!(ir_web[0].path_patterns.len(), 15);

            // clear the active invalidations in the CDN to _fake_ them
            // being completed on the CDN side.
            cdn.clear_active_invalidations();

            // now handle again
            handle_queued_invalidation_requests(
                &env.cdn(),
                &env.metrics(),
                &mut *conn,
                "distribution_id_web",
            )?;

            // which adds the CDN reference
            assert!(queued_or_active_crate_invalidations(&mut *conn)?
                .iter()
                .all(|i| i.cdn_reference.is_some()));

            // and creates them in the CDN too
            let ir_web = cdn.active_invalidations("distribution_id_web")?;
            assert_eq!(ir_web.len(), 1);
            assert_eq!(ir_web[0].path_patterns, vec!["/krate*", "/crate/krate*"]);

            Ok(())
        });
    }

    #[test]
    fn dont_create_invalidations_without_paths() {
        crate::test::wrapper(|env| {
            env.override_config(|config| {
                config.cloudfront_distribution_id_web = Some("distribution_id_web".into());
            });

            let cdn = env.cdn();

            let mut conn = env.db().conn();
            // no invalidation is queued
            assert!(queued_or_active_crate_invalidations(&mut *conn)?.is_empty());

            // run the handler
            handle_queued_invalidation_requests(
                &env.cdn(),
                &env.metrics(),
                &mut *conn,
                "distribution_id_web",
            )?;

            // no invalidation was created
            assert!(cdn.active_invalidations("distribution_id_web")?.is_empty());

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

        CdnBackend::create_cloudfront_invalidation(
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
