use crate::{
    Config,
    index_watcher::{
        process_crate_deleted, process_version_added, process_version_deleted,
        process_version_yank_status,
    },
    metrics::WatcherMetrics,
};
use anyhow::{Context as _, Result};
use aws_config::{BehaviorVersion, Region, retry::RetryConfig};
use aws_sdk_sqs::Client;
use chrono::Utc;
use docs_rs_context::Context;
use docs_rs_crates_io::events::{IndexChangeEventV1, IndexChangeV1};
use docs_rs_types::KrateName;
use docs_rs_utils::retry_async;
use std::time::{Duration, Instant};
use tokio::time;
use tracing::{debug, error, field, instrument, warn};

/// wait-time (long polling):
///
/// How long should the request be kept open when there are no messages.
/// SQS only accepts values in the range 0..=20 seconds.
const WAIT_TIME: Duration = Duration::from_secs(20);

/// when one long-polling request is finished, how long to sleep before starting the next?
const SLEEP_BETWEEN_REQUESTS: Duration = Duration::from_secs(1);

/// when we have an error handling a message, how long should SQS wait until
/// it redelivers this message.
///
/// With FIFO queues, other messages will wait behind.
const RETRY_DELAY: Duration = Duration::from_secs(30);

/// How regularly to recheck the priorities of queued crates.
/// Right now only runs `deprioritize_workspaces`.
const DELAY_BETWEEN_PRIORITY_RECHECK: Duration = Duration::from_secs(60);

/// visibility timeout:
/// SQS visibility timeout is the period after a consumer receives a message during
/// which that message is hidden from other consumers, and if it is not deleted before
/// the timeout expires, it becomes visible again for redelivery.
///
/// Should be longer than the longest time our server takes to handle a message.
const VISIBILITY_TIMEOUT: Duration = Duration::from_secs(60);

/// Result type for `handle_message_body`, so we can unit-test it without needing
/// fake SQS.
#[derive(Debug, Clone, PartialEq, Eq)]
enum MessageOutcome {
    Ack,
    RetryLater(Duration),
    Ignore,
}

pub(crate) async fn run_sqs_subscriber(
    config: &Config,
    context: &Context,
    metrics: &WatcherMetrics,
) -> Result<()> {
    let (Some(region), Some(queue_url)) = (&config.sqs_region, &config.sqs_queue_url) else {
        warn!("missing sqs region or url, disabling crates.io SQS subscriber");
        return Ok(());
    };
    let mut last_priority_recheck = Instant::now();
    let queue = context.build_queue()?;

    debug!("creating SQS client...");
    let shared_config = aws_config::load_defaults(BehaviorVersion::latest()).await;
    let mut client_config = aws_sdk_sqs::config::Builder::from(&shared_config)
        .retry_config(RetryConfig::standard().with_max_attempts(config.aws_sdk_max_retries))
        .region(Region::new(region.to_string()));
    if let Some(endpoint_url) = &config.sqs_endpoint_url {
        client_config = client_config.endpoint_url(endpoint_url.to_string());
    }
    let client = Client::from_conf(client_config.build());

    let queue_url = queue_url.to_string();

    loop {
        if queue.is_locked().await? {
            debug!("Queue is locked, skipping checking new crates");
            time::sleep(WAIT_TIME).await;
            continue;
        }

        debug!("receiving messages...");
        let messages = match client
            .receive_message()
            .queue_url(&queue_url)
            .max_number_of_messages(10)
            .wait_time_seconds(WAIT_TIME.as_secs() as i32)
            .visibility_timeout(VISIBILITY_TIMEOUT.as_secs() as i32)
            .send()
            .await
        {
            Ok(response) => response.messages().to_vec(),
            Err(err) => {
                metrics.sqs_poll_errors_total.add(1, &[]);
                error!(
                    ?err,
                    queue_url, "error receiving messages from sqs, retrying"
                );
                time::sleep(WAIT_TIME).await;
                continue;
            }
        };
        metrics
            .sqs_messages_received_total
            .add(messages.len() as u64, &[]);

        for message in messages {
            match handle_message_body(context, config, metrics, message.body.as_deref()).await {
                MessageOutcome::Ack => {
                    if let Some(receipt_handle) = message.receipt_handle.as_deref()
                        && let Err(err) = client
                            .delete_message()
                            .queue_url(&queue_url)
                            .receipt_handle(receipt_handle)
                            .send()
                            .await
                    {
                        error!(?err, receipt_handle, "error deleting message from queue");
                    }
                }
                MessageOutcome::RetryLater(delay) => {
                    error!(
                        ?message,
                        ?delay,
                        body = message.body.as_deref().unwrap_or_default(),
                        "error handling message. Retrying."
                    );
                    if let Some(receipt_handle) = message.receipt_handle.as_deref()
                        && let Err(err) = client
                            .change_message_visibility()
                            .queue_url(&queue_url)
                            .receipt_handle(receipt_handle)
                            .visibility_timeout(delay.as_secs() as i32)
                            .send()
                            .await
                    {
                        warn!(
                            ?err,
                            receipt_handle, "error setting visibility_timeout for retry"
                        );
                    }
                }
                MessageOutcome::Ignore => {}
            }
        }

        if last_priority_recheck.elapsed() >= DELAY_BETWEEN_PRIORITY_RECHECK {
            if let Err(err) = queue.deprioritize_workspaces().await {
                error!(?err, "error deprioritizing workspaces");
            }

            last_priority_recheck = Instant::now();
        }

        time::sleep(SLEEP_BETWEEN_REQUESTS).await;
    }
}

async fn handle_message_body(
    context: &Context,
    config: &Config,
    metrics: &WatcherMetrics,
    body: Option<&str>,
) -> MessageOutcome {
    let Some(body) = body else {
        return MessageOutcome::Ignore;
    };
    let start = Instant::now();

    match retry_async(
        || async move { process_sqs_event(context, config, metrics, body).await },
        3,
    )
    .await
    {
        Ok(_) => {
            metrics
                .sqs_message_processing_time
                .record(start.elapsed().as_secs_f64(), &[]);
            MessageOutcome::Ack
        }
        Err(err) => {
            metrics
                .sqs_message_processing_time
                .record(start.elapsed().as_secs_f64(), &[]);
            metrics.sqs_retries_total.add(1, &[]);
            error!(
                ?err,
                ?RETRY_DELAY,
                body,
                "error handling message. Retrying."
            );
            MessageOutcome::RetryLater(RETRY_DELAY)
        }
    }
}

#[instrument(skip_all, fields(change_type = field::Empty, krate = field::Empty))]
async fn process_sqs_event(
    context: &Context,
    config: &Config,
    metrics: &WatcherMetrics,
    body: &str,
) -> Result<()> {
    let event: IndexChangeEventV1 =
        serde_json::from_str(body).context("error parsing event from json")?;

    {
        let span = tracing::Span::current();
        span.record("change_type", event.change.kind());
        span.record("krate", event.change.name());
    }

    debug!(?event, "received event from sqs");
    metrics
        .sqs_event_lag
        .record((Utc::now() - event.occurred_at).as_seconds_f64(), &[]);

    if config.sqs_active {
        process_change(context, &event.change, config)
            .await
            .context("error processing change")?;
        metrics.record_change_applied(&event.change);
    }

    Ok(())
}

/// Process a crate change
#[instrument(skip(context, config))]
pub(crate) async fn process_change(
    context: &Context,
    change: &IndexChangeV1,
    config: &Config,
) -> Result<()> {
    match change {
        IndexChangeV1::Added(crate_version) => {
            process_version_added(context, &crate_version.try_into()?).await?
        }
        IndexChangeV1::Yanked(crate_version) => {
            process_version_yank_status(context, &crate_version.try_into()?, true).await?
        }
        IndexChangeV1::Unyanked(crate_version) => {
            process_version_yank_status(context, &crate_version.try_into()?, false).await?
        }
        IndexChangeV1::CrateDeleted { name, .. } => {
            let name: KrateName = name.parse()?;
            process_crate_deleted(context, config, &name).await?
        }
        IndexChangeV1::VersionDeleted(crate_version) => {
            process_version_deleted(context, config, &crate_version.try_into()?).await?
        }
    };
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::TestEnvironment;
    use docs_rs_config::AppConfig as _;
    use docs_rs_crates_io::events::CrateVersion;
    use docs_rs_types::{
        Version,
        testing::{KRATE, V1, V2},
    };
    use pretty_assertions::assert_eq;

    fn added_event_json(name: &KrateName, version: &Version) -> String {
        serde_json::to_string(&serde_json::json!({
            "id":"evt_123",
            "occurred_at":"2026-06-01T12:00:00Z",
            "type":"added",
            "payload":{
                "name": name.to_string(),
                "vers": version.to_string(),
            }
        }))
        .unwrap()
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_process_change_added_queues_crate() -> Result<()> {
        let env = TestEnvironment::new().await?;

        process_change(
            &env,
            &IndexChangeV1::Added(CrateVersion {
                name: KRATE.to_string(),
                version: V1.to_string(),
            }),
            env.config(),
        )
        .await?;

        let queue = env.build_queue()?.queued_crates().await?;
        assert_eq!(queue.len(), 1);
        assert_eq!(queue[0].name, KRATE);
        assert_eq!(queue[0].version, V1);

        Ok(())
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_process_change_yanked_updates_release() -> Result<()> {
        let env = TestEnvironment::new().await?;
        let mut conn = env.async_conn().await?;

        let id = env
            .fake_release()
            .await
            .name(KRATE)
            .version(V1)
            .create()
            .await?;

        process_change(
            &env,
            &IndexChangeV1::Yanked(CrateVersion {
                name: KRATE.to_string(),
                version: V1.to_string(),
            }),
            env.config(),
        )
        .await?;

        let yanked = sqlx::query_scalar!(
            "SELECT yanked
             FROM releases
             WHERE id = $1",
            id.0
        )
        .fetch_one(&mut *conn)
        .await?;
        assert_eq!(yanked, Some(true));

        Ok(())
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_process_change_version_deleted_removes_release() -> Result<()> {
        let env = TestEnvironment::new().await?;
        let mut conn = env.async_conn().await?;

        let rid_1 = env
            .fake_release()
            .await
            .name(KRATE)
            .version(V1)
            .create()
            .await?;
        env.fake_release()
            .await
            .name(KRATE)
            .version(V2)
            .create()
            .await?;

        process_change(
            &env,
            &IndexChangeV1::VersionDeleted(CrateVersion {
                name: KRATE.to_string(),
                version: V2.to_string(),
            }),
            env.config(),
        )
        .await?;

        let rows = sqlx::query_scalar!(
            "SELECT id
             FROM releases",
        )
        .fetch_all(&mut *conn)
        .await?;
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0], rid_1.0);

        Ok(())
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_process_sqs_event_dispatches_added_event() -> Result<()> {
        let mut config = Config::test_config()?;
        config.sqs_active = true;
        let env = TestEnvironment::builder().config(config).build().await?;
        let metrics = WatcherMetrics::new(&env.context().meter_provider);

        process_sqs_event(&env, env.config(), &metrics, &added_event_json(&KRATE, &V1)).await?;

        let queue = env.build_queue()?.queued_crates().await?;
        assert_eq!(queue.len(), 1);
        assert_eq!(queue[0].name, KRATE);
        assert_eq!(queue[0].version, V1);
        let collected = env.collected_metrics();
        let applied_metric =
            collected.get_metric("watcher", "docsrs.watcher.changes_applied_total")?;
        let applied = applied_metric.get_u64_counter();
        let change_type = applied
            .attributes()
            .find(|kv| kv.key.as_str() == "type")
            .unwrap()
            .value
            .to_string();
        assert_eq!(change_type, "added");
        assert_eq!(applied.value(), 1);
        let lag_metric = collected.get_metric("watcher", "docsrs.watcher.sqs_event_lag_seconds")?;
        assert_eq!(lag_metric.get_f64_histogram().count(), 1);

        Ok(())
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_process_sqs_event_respects_sqs_active() -> Result<()> {
        let mut config = Config::test_config()?;
        config.sqs_active = false;
        let env = TestEnvironment::builder().config(config).build().await?;
        let metrics = WatcherMetrics::new(&env.context().meter_provider);

        process_sqs_event(&env, env.config(), &metrics, &added_event_json(&KRATE, &V1)).await?;

        assert!(env.build_queue()?.queued_crates().await?.is_empty());

        Ok(())
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_process_sqs_event_rejects_invalid_json() -> Result<()> {
        let env = TestEnvironment::new().await?;
        let metrics = WatcherMetrics::new(&env.context().meter_provider);

        let err = process_sqs_event(&env, env.config(), &metrics, "{not json").await;

        assert!(err.is_err());
        let err = format!("{:?}", err.unwrap_err());
        assert!(
            err.contains("error parsing event from json"),
            "unexpected error: {err}"
        );

        Ok(())
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_handle_message_body_acknowledges_success() -> Result<()> {
        let config = Config::test_config()?;
        let env = TestEnvironment::builder().config(config).build().await?;
        let metrics = WatcherMetrics::new(&env.context().meter_provider);

        assert_eq!(
            handle_message_body(
                &env,
                env.config(),
                &metrics,
                Some(&added_event_json(&KRATE, &V1)),
            )
            .await,
            MessageOutcome::Ack
        );
        let collected = env.collected_metrics();
        let processing_metric =
            collected.get_metric("watcher", "docsrs.watcher.sqs_message_processing_seconds")?;
        assert_eq!(processing_metric.get_f64_histogram().count(), 1);

        Ok(())
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_handle_message_body_retries_failed_processing() -> Result<()> {
        let env = TestEnvironment::new().await?;
        let metrics = WatcherMetrics::new(&env.context().meter_provider);

        assert_eq!(
            handle_message_body(&env, env.config(), &metrics, Some("{bad json")).await,
            MessageOutcome::RetryLater(RETRY_DELAY)
        );
        let collected = env.collected_metrics();
        assert_eq!(
            collected
                .get_metric("watcher", "docsrs.watcher.sqs_retries_total")?
                .get_u64_counter()
                .value(),
            1
        );

        Ok(())
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_handle_message_body_ignores_missing_body() -> Result<()> {
        let env = TestEnvironment::new().await?;
        let metrics = WatcherMetrics::new(&env.context().meter_provider);

        assert_eq!(
            handle_message_body(&env, env.config(), &metrics, None).await,
            MessageOutcome::Ignore
        );
        assert!(env.build_queue()?.queued_crates().await?.is_empty());

        Ok(())
    }
}
