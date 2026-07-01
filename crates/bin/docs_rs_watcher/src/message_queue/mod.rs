use anyhow::Result;
use std::time::Duration;

pub(crate) mod sqs;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ReceivedMessage {
    pub(crate) body: Option<String>,
    pub(crate) receipt_handle: Option<String>,
}

#[async_trait::async_trait]
pub(crate) trait MessageQueueClient: Sync {
    async fn receive_messages(&self) -> Result<Vec<ReceivedMessage>>;
    async fn delete_message(&self, receipt_handle: &str) -> Result<()>;
    async fn retry_message(&self, receipt_handle: &str, delay: Duration) -> Result<()>;
}
