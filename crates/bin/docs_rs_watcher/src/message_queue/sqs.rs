use crate::{
    message_queue::{MessageQueueClient, ReceivedMessage},
    subscriber::WAIT_TIME,
};
use anyhow::Result;
use aws_config::{BehaviorVersion, Region, retry::RetryConfig};
use aws_sdk_sqs::Client;
use futures_util::future::BoxFuture;
use std::time::Duration;
use url::Url;

/// visibility timeout:
/// should be longer than the longest time our server takes to handle a message.
///
/// if we fetch a message, and don't delete it in this time, it will be redelivered.
const VISIBILITY_TIMEOUT: Duration = Duration::from_secs(60);

pub(crate) struct AwsSqsClient {
    inner: Client,
    queue_url: String,
}

impl AwsSqsClient {
    pub(crate) async fn new(queue_url: &Url, region: impl Into<String>, max_retries: u32) -> Self {
        let shared_config = aws_config::load_defaults(BehaviorVersion::latest()).await;
        Self {
            queue_url: queue_url.to_string(),
            inner: Client::from_conf(
                aws_sdk_sqs::config::Builder::from(&shared_config)
                    .retry_config(RetryConfig::standard().with_max_attempts(max_retries))
                    .region(Region::new(region.into()))
                    .build(),
            ),
        }
    }
}

impl MessageQueueClient for AwsSqsClient {
    fn receive_messages<'a>(&'a self) -> BoxFuture<'a, Result<Vec<ReceivedMessage>>> {
        Box::pin(async move {
            let response = self
                .inner
                .receive_message()
                .queue_url(&self.queue_url)
                .max_number_of_messages(10)
                .wait_time_seconds(WAIT_TIME.as_secs() as i32)
                .visibility_timeout(VISIBILITY_TIMEOUT.as_secs() as i32)
                .send()
                .await?;

            Ok(response
                .messages()
                .iter()
                .map(|message| ReceivedMessage {
                    body: message.body().map(str::to_owned),
                    receipt_handle: message.receipt_handle().map(str::to_owned),
                })
                .collect())
        })
    }

    fn delete_message<'a>(&'a self, receipt_handle: &'a str) -> BoxFuture<'a, Result<()>> {
        Box::pin(async move {
            self.inner
                .delete_message()
                .queue_url(&self.queue_url)
                .receipt_handle(receipt_handle)
                .send()
                .await?;
            Ok(())
        })
    }

    fn retry_message<'a>(
        &'a self,
        receipt_handle: &'a str,
        delay: Duration,
    ) -> BoxFuture<'a, Result<()>> {
        Box::pin(async move {
            self.inner
                .change_message_visibility()
                .queue_url(&self.queue_url)
                .receipt_handle(receipt_handle)
                .visibility_timeout(delay.as_secs() as i32)
                .send()
                .await?;
            Ok(())
        })
    }
}
