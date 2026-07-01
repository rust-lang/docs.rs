use crate::message_queue::{MessageQueueClient, ReceivedMessage};
use anyhow::{Result, anyhow};
use std::{
    sync::{Arc, Mutex},
    time::Duration,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum FakeAction {
    Delete {
        receipt_handle: String,
    },
    Retry {
        receipt_handle: String,
        delay: Duration,
    },
}

#[derive(Clone)]
pub(crate) struct FakeMessageQueueClient {
    pub(crate) receive_result: Arc<Mutex<Result<Vec<ReceivedMessage>, String>>>,
    pub(crate) actions: Arc<Mutex<Vec<FakeAction>>>,
    pub(crate) delete_error: Arc<Mutex<Option<String>>>,
    pub(crate) retry_error: Arc<Mutex<Option<String>>>,
}

impl FakeMessageQueueClient {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    pub(crate) fn with_messages(messages: Vec<ReceivedMessage>) -> Self {
        Self {
            receive_result: Arc::new(Mutex::new(Ok(messages))),
            ..Self::default()
        }
    }
}

impl Default for FakeMessageQueueClient {
    fn default() -> Self {
        Self {
            receive_result: Arc::new(Mutex::new(Ok(Vec::new()))),
            actions: Arc::new(Mutex::new(Vec::new())),
            delete_error: Arc::new(Mutex::new(None)),
            retry_error: Arc::new(Mutex::new(None)),
        }
    }
}

#[async_trait::async_trait]
impl MessageQueueClient for FakeMessageQueueClient {
    async fn receive_messages(&self) -> Result<Vec<ReceivedMessage>> {
        self.receive_result
            .lock()
            .unwrap()
            .clone()
            .map_err(|err| anyhow!(err))
    }

    async fn delete_message(&self, receipt_handle: &str) -> Result<()> {
        self.actions.lock().unwrap().push(FakeAction::Delete {
            receipt_handle: receipt_handle.to_string(),
        });
        if let Some(err) = self.delete_error.lock().unwrap().clone() {
            Err(anyhow!(err))
        } else {
            Ok(())
        }
    }

    async fn retry_message(&self, receipt_handle: &str, delay: Duration) -> Result<()> {
        self.actions.lock().unwrap().push(FakeAction::Retry {
            receipt_handle: receipt_handle.to_string(),
            delay,
        });
        if let Some(err) = self.retry_error.lock().unwrap().clone() {
            Err(anyhow!(err))
        } else {
            Ok(())
        }
    }
}
