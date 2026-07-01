use crate::{
    Config,
    index_watcher::{
        process_crate_deleted, process_version_added, process_version_deleted,
        process_version_yank_status,
    },
    synchronization::CrateLocks,
};
use anyhow::{Context as _, Result};
use aws_config::{BehaviorVersion, Region, retry::RetryConfig};
use aws_sdk_sqs::Client;
use docs_rs_context::Context;
use docs_rs_crates_io::events::{IndexChangeEventV1, IndexChangeV1};
use docs_rs_types::KrateName;
use docs_rs_utils::retry_async;
use std::time::{Duration, Instant};
use tokio::time;
use tracing::{debug, error, instrument, warn};

// TODO:
// * when should we run deprioritize_workspaces ?

/// visibility timeout:
/// should be longer than the longest time our server takes to handle a message.
///
/// if we fetch a message, and don't delete it in this time, it will be redelivered.
const VISIBILITY_TIMEOUT: Duration = Duration::from_secs(60);

/// wait-time (long polling):
///
/// How long should the request be kept open when there are no messages.
const WAIT_TIME: Duration = Duration::from_secs(30);

/// when one long-polling request is finished, how long to sleep before starting the next?
const SLEEP_BETWEEN_REQUESTS: Duration = Duration::from_secs(1);

/// when we have an error handling a message, how long should SQS wait until
/// it redelivers this message.
const RETRY_DELAY: Duration = Duration::from_secs(30);

/// How long to wait before rechecking the priorities of queued crates.
const DELAY_BETWEEN_PRIORITY_RECHECK: Duration = Duration::from_secs(60);

pub async fn listen(config: &Config, context: &Context, locks: &CrateLocks) -> Result<()> {
    let (Some(region), Some(queue_url)) = (&config.sqs_region, &config.sqs_queue_url) else {
        warn!("missing sqs region or url, disabling crates.io SQS subscriber");
        return Ok(());
    };

    let queue_url = queue_url.to_string();

    let shared_config = aws_config::load_defaults(BehaviorVersion::latest()).await;
    let client = Client::from_conf(
        aws_sdk_sqs::config::Builder::from(&shared_config)
            .retry_config(RetryConfig::standard().with_max_attempts(config.aws_sdk_max_retries))
            .region(Region::new(region.clone()))
            .build(),
    );

    let mut last_priority_recheck = Instant::now();
    let queue = context.build_queue()?;

    loop {
        if queue.is_locked().await? {
            debug!("Queue is locked, skipping checking new crates");
            time::sleep(WAIT_TIME).await;
            continue;
        }

        let response = match client
            .receive_message()
            .queue_url(queue_url.clone())
            .max_number_of_messages(10)
            .wait_time_seconds(WAIT_TIME.as_secs() as i32)
            .visibility_timeout(VISIBILITY_TIMEOUT.as_secs() as i32)
            .send()
            .await
        {
            Ok(response) => response,
            Err(err) => {
                error!(
                    ?err,
                    queue_url, "error receiving messages from sqs, retrying"
                );
                time::sleep(WAIT_TIME).await;
                continue;
            }
        };

        for message in response.messages() {
            let Some(body) = message.body() else {
                continue;
            };

            match retry_async(
                || async move { process_message(context, config, locks, body).await },
                3,
            )
            .await
            {
                Ok(_) => {
                    if let Some(receipt_handle) = message.receipt_handle() {
                        // mark the message as "done"
                        if let Err(err) = client
                            .delete_message()
                            .queue_url(queue_url.clone())
                            .receipt_handle(receipt_handle)
                            .send()
                            .await
                        {
                            // sqs will redeliver the message after the visibility timeout passed
                            error!(
                                ?err,
                                receipt_handle, queue_url, "error deleting message from queue"
                            );
                        }
                    }
                }
                Err(err) => {
                    error!(
                        ?err,
                        ?message,
                        ?RETRY_DELAY,
                        body,
                        "error handling message. Retrying."
                    );

                    if let Some(receipt_handle) = message.receipt_handle() {
                        // Don't delete the message.
                        // It will become visible again after the visibility timeout.
                        if let Err(err) = client
                            .change_message_visibility()
                            .queue_url(queue_url.clone())
                            .receipt_handle(receipt_handle)
                            // retry after some time
                            .visibility_timeout(RETRY_DELAY.as_secs() as i32) // retry
                            .send()
                            .await
                        {
                            // this error doesn't really matter, without the changed visibility
                            // timeout sqs will redeliver after the default visibility timeout.
                            warn!(
                                ?err,
                                receipt_handle,
                                queue_url,
                                "error setting visibility_timeout for retry"
                            );
                        }
                    }
                }
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

#[instrument(skip(context, config, locks))]
async fn process_message(
    context: &Context,
    config: &Config,
    locks: &CrateLocks,
    body: &str,
) -> Result<()> {
    let event: IndexChangeEventV1 =
        serde_json::from_str(body).context("error parsing event from json")?;

    debug!(?event, "received event from sqs");

    let _guard = locks.lock(event.change.name()).await;

    if !config.sqs_dry_run {
        process_change(context, &event.change, config)
            .await
            .context("error processing change")?;
    }

    Ok(())
}

/// Process a crate change, returning whether the change was a crate addition or not.
pub(crate) async fn process_change(
    context: &Context,
    change: &IndexChangeV1,
    config: &Config,
) -> Result<bool> {
    match change {
        IndexChangeV1::Added(crate_version) => {
            process_version_added(context, &crate_version.try_into().unwrap()).await?
        }
        IndexChangeV1::Yanked(crate_version) => {
            process_version_yank_status(context, &crate_version.try_into().unwrap(), true).await?
        }
        IndexChangeV1::Unyanked(crate_version) => {
            process_version_yank_status(context, &crate_version.try_into().unwrap(), false).await?
        }
        IndexChangeV1::CrateDeleted { name, .. } => {
            let name: KrateName = name.parse()?;
            process_crate_deleted(context, config, &name).await?
        }
        IndexChangeV1::VersionDeleted(crate_version) => {
            process_version_deleted(context, config, &crate_version.try_into().unwrap()).await?
        }
    };
    Ok(change.added().is_some())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::TestEnvironment;
    use docs_rs_config::AppConfig as _;
    use docs_rs_crates_io::events::CrateVersion;
    use docs_rs_types::testing::{KRATE, V1, V2};
    use pretty_assertions::assert_eq;

    fn added_event_json(name: &str, version: &str) -> String {
        format!(
            r#"{{"id":"evt_123","occurred_at":"2026-06-01T12:00:00Z","type":"added","payload":{{"name":"{name}","vers":"{version}"}}}}"#
        )
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_process_change_added_queues_crate() -> Result<()> {
        let env = TestEnvironment::new().await?;

        let added = process_change(
            &env,
            &IndexChangeV1::Added(CrateVersion {
                name: KRATE.to_string(),
                version: V1.to_string(),
            }),
            env.config(),
        )
        .await?;

        assert!(added);
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
            .name("krate")
            .version(V1)
            .create()
            .await?;

        let added = process_change(
            &env,
            &IndexChangeV1::Yanked(CrateVersion {
                name: KRATE.to_string(),
                version: V1.to_string(),
            }),
            env.config(),
        )
        .await?;

        assert!(!added);
        let row = sqlx::query!(
            "SELECT yanked
             FROM releases
             WHERE id = $1",
            id.0
        )
        .fetch_one(&mut *conn)
        .await?;
        assert_eq!(row.yanked, Some(true));

        Ok(())
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_process_change_version_deleted_removes_release() -> Result<()> {
        let env = TestEnvironment::new().await?;
        let mut conn = env.async_conn().await?;

        let rid_1 = env
            .fake_release()
            .await
            .name("krate")
            .version(V1)
            .create()
            .await?;
        env.fake_release()
            .await
            .name("krate")
            .version(V2)
            .create()
            .await?;

        let added = process_change(
            &env,
            &IndexChangeV1::VersionDeleted(CrateVersion {
                name: KRATE.to_string(),
                version: V2.to_string(),
            }),
            env.config(),
        )
        .await?;

        assert!(!added);
        let rows = sqlx::query!(
            "SELECT id
             FROM releases",
        )
        .fetch_all(&mut *conn)
        .await?;
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].id, rid_1.0);

        Ok(())
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_process_message_dispatches_added_event() -> Result<()> {
        let mut config = Config::test_config()?;
        config.sqs_dry_run = false;
        let env = TestEnvironment::builder().config(config).build().await?;

        process_message(
            &env,
            env.config(),
            &CrateLocks::new(),
            &added_event_json("krate", &V1.to_string()),
        )
        .await?;

        let queue = env.build_queue()?.queued_crates().await?;
        assert_eq!(queue.len(), 1);
        assert_eq!(queue[0].name, KRATE);
        assert_eq!(queue[0].version, V1);

        Ok(())
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_process_message_respects_sqs_dry_run() -> Result<()> {
        let env = TestEnvironment::new().await?;

        process_message(
            &env,
            env.config(),
            &CrateLocks::new(),
            &added_event_json("krate", &V1.to_string()),
        )
        .await?;

        assert!(env.build_queue()?.queued_crates().await?.is_empty());

        Ok(())
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_process_message_rejects_invalid_json() -> Result<()> {
        let env = TestEnvironment::new().await?;

        let err = process_message(&env, env.config(), &CrateLocks::new(), "{not json").await;

        assert!(err.is_err());
        let err = format!("{:?}", err.unwrap_err());
        assert!(
            err.contains("error parsing event from json"),
            "unexpected error: {err}"
        );

        Ok(())
    }
}
