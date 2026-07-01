use anyhow::Result;
use futures_util::future::BoxFuture;
use std::time::Duration;

pub(crate) mod sqs;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ReceivedMessage {
    pub(crate) body: Option<String>,
    pub(crate) receipt_handle: Option<String>,
}

pub(crate) trait MessageQueueClient: Sync {
    fn receive_messages<'a>(&'a self) -> BoxFuture<'a, Result<Vec<ReceivedMessage>>>;
    fn delete_message<'a>(&'a self, receipt_handle: &'a str) -> BoxFuture<'a, Result<()>>;
    fn retry_message<'a>(
        &'a self,
        receipt_handle: &'a str,
        delay: Duration,
    ) -> BoxFuture<'a, Result<()>>;
}
