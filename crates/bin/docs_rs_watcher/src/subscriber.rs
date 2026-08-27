use crate::{
    Config,
    index_watcher::{
        process_crate_deleted, process_version_added, process_version_deleted,
        process_version_yank_status,
    },
    metrics::{EventSource, WatcherMetrics},
};
use anyhow::{Context as _, Result};
use aws_config::{BehaviorVersion, Region, retry::RetryConfig};
use aws_sdk_sqs::{Client, types::Message};
use chrono::Utc;
use docs_rs_context::Context;
use docs_rs_crates_io::events::{IndexChangeEventV1, IndexChangeV1};
use docs_rs_types::KrateName;
use docs_rs_utils::retry_async;
use std::time::{Duration, Instant};
use tokio::time;
use tracing::{debug, error, instrument, warn};

/// wait-time (long polling):
///
/// How long should the request be kept open when there are no messages.
/// SQS only accepts values in the range 0..=20 seconds.
const WAIT_TIME: Duration = Duration::from_secs(20);

/// when one long-polling request is finished, how long to sleep before starting the next?
const SLEEP_BETWEEN_REQUESTS: Duration = Duration::from_secs(1);

/// How regularly to recheck the priorities of queued crates.
/// Right now only runs `deprioritize_workspaces`.
const DELAY_BETWEEN_PRIORITY_RECHECK: Duration = Duration::from_secs(60);

/// visibility timeout:
/// SQS visibility timeout is the period after a consumer receives a message during
/// which that message is hidden from other consumers, and if it is not deleted before
/// the timeout expires, it becomes visible again for redelivery.
///
/// Should be longer than the longest time our server takes to handle a message.
const VISIBILITY_TIMEOUT: Duration = Duration::from_secs(600);

trait SqsActions {
    async fn delete_message(&self, queue_url: &str, receipt_handle: &str) -> Result<()>;
}

impl SqsActions for Client {
    async fn delete_message(&self, queue_url: &str, receipt_handle: &str) -> Result<()> {
        self.delete_message()
            .queue_url(queue_url)
            .receipt_handle(receipt_handle)
            .send()
            .await
            .context("error deleting SQS message")?;
        Ok(())
    }
}

pub(crate) async fn run_sqs_subscriber(
    config: &Config,
    context: &Context,
    metrics: &WatcherMetrics,
) -> Result<()> {
    let Some(sqs_config) = &config.crates_io_events else {
        warn!("missing sqs config, disabling crates.io SQS subscriber");
        return Ok(());
    };
    let mut last_priority_recheck = Instant::now();
    let queue = context.build_queue()?;

    debug!("creating SQS client...");
    let shared_config = aws_config::load_defaults(BehaviorVersion::latest()).await;
    let mut client_config = aws_sdk_sqs::config::Builder::from(&shared_config)
        .retry_config(RetryConfig::standard().with_max_attempts(sqs_config.max_retries))
        .region(Region::new(sqs_config.region.to_string()));
    if let Some(endpoint_url) = &sqs_config.endpoint_url {
        client_config = client_config.endpoint_url(endpoint_url.to_string());
    }
    let client = Client::from_conf(client_config.build());

    let queue_url = sqs_config.queue_url.to_string();

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
            // confirm that we want to do batches.
            // important because it's a FIFO queue:
            // NOTE: when we start retrying tasks with a FIFO queute.
            // important: return on on the first erroring message, don't
            // handle the rest of the batch.
            .max_number_of_messages(10)
            .wait_time_seconds(WAIT_TIME.as_secs() as i32)
            .visibility_timeout(VISIBILITY_TIMEOUT.as_secs() as i32)
            .send()
            .await
        {
            Ok(response) => response.messages().to_vec(),
            Err(err) => {
                // NOTE: right now we handle the change-events like the old
                // git index: on error just skip over the event, handle the next.
                // Future improvement: retry the task for retryable errors.
                metrics.record_poll_error(EventSource::Sqs);
                error!(?err, queue_url, "error receiving messages from sqs");
                time::sleep(WAIT_TIME).await;
                continue;
            }
        };
        process_messages(&client, &queue_url, context, config, metrics, messages).await;

        if last_priority_recheck.elapsed() >= DELAY_BETWEEN_PRIORITY_RECHECK {
            if let Err(err) = queue.reevaluate_priorities().await {
                error!(?err, "error reevaluating queued release priorities");
            }

            last_priority_recheck = Instant::now();
        }

        time::sleep(SLEEP_BETWEEN_REQUESTS).await;
    }
}

async fn process_messages(
    client: &impl SqsActions,
    queue_url: &str,
    context: &Context,
    config: &Config,
    metrics: &WatcherMetrics,
    messages: Vec<Message>,
) {
    for message in messages {
        handle_message_body(context, config, metrics, message.body.as_deref()).await;
        if let Some(receipt_handle) = message.receipt_handle.as_deref()
            && let Err(err) = client.delete_message(queue_url, receipt_handle).await
        {
            error!(?err, receipt_handle, "error deleting message from queue");
        }
    }
}

async fn handle_message_body(
    context: &Context,
    config: &Config,
    metrics: &WatcherMetrics,
    body: Option<&str>,
) {
    let Some(body) = body else {
        return;
    };
    if let Err(err) = process_sqs_event(context, config, metrics, body).await {
        // Match the git-index watcher behavior for the initial rollout: record and skip
        // failed events instead of letting one event block the FIFO queue indefinitely.
        error!(?err, body, "error handling message, skipping event");
    }
}

#[instrument(skip_all)]
async fn process_sqs_event(
    context: &Context,
    config: &Config,
    metrics: &WatcherMetrics,
    body: &str,
) -> Result<()> {
    metrics.record_events_received(EventSource::Sqs, 1);

    let start = Instant::now();
    let event: IndexChangeEventV1 = match serde_json::from_str(body) {
        Ok(event) => event,
        Err(err) => {
            metrics.record_event_processing_time(EventSource::Sqs, None, false, start.elapsed());
            return Err(err).context("error parsing event from json");
        }
    };

    debug!(
        target: "docs_rs_watcher::index_event",
        source = %EventSource::Sqs,
        event_id = %event.id,
        occurred_at = %event.occurred_at,
        change_type = %event.change.kind(),
        crate_name = event.change.name(),
        crate_version = event.change.version().unwrap_or_default(),
        "crates.io index event"
    );

    if let Ok(lag) = (Utc::now() - event.occurred_at).to_std() {
        metrics.record_event_lag(EventSource::Sqs, lag);
    }

    let processing_result = if config.crates_io_events_active() {
        retry_async(
            || {
                let change = event.change.clone();
                async move { process_change(context, &change, config).await }
            },
            3,
        )
        .await
        .context("error processing change")
    } else {
        Ok(())
    };

    metrics.record_event_processing_time(
        EventSource::Sqs,
        Some(event.change.kind()),
        processing_result.is_ok(),
        start.elapsed(),
    );
    processing_result?;

    if config.crates_io_events_active() {
        metrics.record_change_applied(EventSource::Sqs, event.change.kind());
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
    use std::sync::Mutex;

    #[derive(Default)]
    struct FakeSqsActions {
        deleted: Mutex<Vec<String>>,
    }

    impl SqsActions for FakeSqsActions {
        async fn delete_message(&self, _queue_url: &str, receipt_handle: &str) -> Result<()> {
            self.deleted.lock().unwrap().push(receipt_handle.into());
            Ok(())
        }
    }

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
    async fn test_process_change_unyanked_updates_release() -> Result<()> {
        let env = TestEnvironment::new().await?;
        let mut conn = env.async_conn().await?;

        let id = env
            .fake_release()
            .await
            .name(KRATE)
            .version(V1)
            .yanked(true)
            .create()
            .await?;

        process_change(
            &env,
            &IndexChangeV1::Unyanked(CrateVersion {
                name: KRATE.to_string(),
                version: V1.to_string(),
            }),
            env.config(),
        )
        .await?;

        let row = sqlx::query!(
            "SELECT yanked
             FROM releases
             WHERE id = $1",
            id.0
        )
        .fetch_one(&mut *conn)
        .await?;
        assert_eq!(row.yanked, Some(false));

        Ok(())
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_process_change_crate_deleted_removes_crate() -> Result<()> {
        let env = TestEnvironment::new().await?;
        let mut conn = env.async_conn().await?;

        env.fake_release()
            .await
            .name(KRATE)
            .version(V1)
            .create()
            .await?;

        process_change(
            &env,
            &IndexChangeV1::CrateDeleted {
                name: KRATE.to_string(),
            },
            env.config(),
        )
        .await?;

        let row = sqlx::query!(
            "SELECT id
             FROM crates
             WHERE name = $1",
            KRATE as _
        )
        .fetch_optional(&mut *conn)
        .await?;
        assert!(row.is_none());

        Ok(())
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_process_change_added_is_idempotent() -> Result<()> {
        let env = TestEnvironment::new().await?;
        let change = IndexChangeV1::Added(CrateVersion {
            name: KRATE.to_string(),
            version: V1.to_string(),
        });

        process_change(&env, &change, env.config()).await?;
        process_change(&env, &change, env.config()).await?;

        let queue = env.build_queue()?.queued_crates().await?;
        assert_eq!(queue.len(), 1);
        assert_eq!(queue[0].name, KRATE);
        assert_eq!(queue[0].version, V1);

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

        assert_eq!(
            sqlx::query_scalar!("SELECT id FROM releases")
                .fetch_all(&mut *conn)
                .await?,
            vec![rid_1.0]
        );

        Ok(())
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_process_change_version_deleted_is_idempotent() -> Result<()> {
        let env = TestEnvironment::new().await?;
        env.fake_release()
            .await
            .name(KRATE)
            .version(V1)
            .create()
            .await?;
        let change = IndexChangeV1::VersionDeleted(CrateVersion {
            name: KRATE.to_string(),
            version: V1.to_string(),
        });

        process_change(&env, &change, env.config()).await?;
        process_change(&env, &change, env.config()).await?;

        assert!(
            sqlx::query_scalar!("SELECT id FROM releases")
                .fetch_all(&mut *env.async_conn().await?)
                .await?
                .is_empty()
        );

        Ok(())
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_process_sqs_event_dispatches_added_event() -> Result<()> {
        let mut config = Config::test_config()?;
        if let Some(sqs_config) = &mut config.crates_io_events {
            sqs_config.active = true;
        }
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
        let lag_metric = collected.get_metric("watcher", "docsrs.watcher.event_lag")?;
        assert_eq!(lag_metric.get_f64_histogram().count(), 1);

        Ok(())
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_process_sqs_event_respects_sqs_active() -> Result<()> {
        let mut config = Config::test_config()?;
        if let Some(sqs_config) = &mut config.crates_io_events {
            sqs_config.active = false;
        }
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

        handle_message_body(
            &env,
            env.config(),
            &metrics,
            Some(&added_event_json(&KRATE, &V1)),
        )
        .await;
        let collected = env.collected_metrics();
        let processing_metric =
            collected.get_metric("watcher", "docsrs.watcher.event_processing_time")?;
        assert_eq!(processing_metric.get_f64_histogram().count(), 1);

        Ok(())
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_handle_message_body_records_failed_processing() -> Result<()> {
        let env = TestEnvironment::new().await?;
        let metrics = WatcherMetrics::new(&env.context().meter_provider);

        handle_message_body(&env, env.config(), &metrics, Some("{bad json")).await;
        let collected = env.collected_metrics();
        let processing_metric =
            collected.get_metric("watcher", "docsrs.watcher.event_processing_time")?;
        let processing = processing_metric.get_f64_histogram();
        assert_eq!(processing.count(), 1);
        assert!(processing.attributes().any(|attribute| {
            attribute.key.as_str() == "result" && attribute.value.to_string() == "err"
        }));

        Ok(())
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_handle_message_body_acknowledges_missing_body() -> Result<()> {
        let env = TestEnvironment::new().await?;
        let metrics = WatcherMetrics::new(&env.context().meter_provider);

        handle_message_body(&env, env.config(), &metrics, None).await;
        assert!(env.build_queue()?.queued_crates().await?.is_empty());

        Ok(())
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_process_messages_skips_errors_and_continues_batch() -> Result<()> {
        let config = Config::test_config()?;
        let env = TestEnvironment::builder().config(config).build().await?;
        let metrics = WatcherMetrics::new(&env.context().meter_provider);
        let client = FakeSqsActions::default();
        let messages = vec![
            Message::builder()
                .body(added_event_json(&KRATE, &V1))
                .receipt_handle("success-1")
                .build(),
            Message::builder()
                .body("{bad json")
                .receipt_handle("failure")
                .build(),
            Message::builder()
                .body(added_event_json(&KRATE, &V2))
                .receipt_handle("success-2")
                .build(),
        ];

        process_messages(&client, "queue-url", &env, env.config(), &metrics, messages).await;

        assert_eq!(
            *client.deleted.lock().unwrap(),
            vec!["success-1", "failure", "success-2"]
        );

        Ok(())
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_process_messages_without_body_is_acknowledged() -> Result<()> {
        let config = Config::test_config()?;
        let env = TestEnvironment::builder().config(config).build().await?;
        let metrics = WatcherMetrics::new(&env.context().meter_provider);
        let client = FakeSqsActions::default();

        process_messages(
            &client,
            "queue-url",
            &env,
            env.config(),
            &metrics,
            vec![Message::builder().receipt_handle("missing-body").build()],
        )
        .await;

        assert_eq!(
            *client.deleted.lock().unwrap(),
            vec!["missing-body".to_string()]
        );
        Ok(())
    }
}
