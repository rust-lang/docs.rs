use std::{collections::HashMap, sync::Arc};
use tokio::sync::{Mutex, OwnedMutexGuard};

/// shared locks so we can serialize changes to the same crate,
/// for the transition phase where we might get input from both
/// the git index and the sqs queue.
#[derive(Clone, Default)]
pub struct CrateLocks {
    locks: Arc<Mutex<HashMap<String, Arc<Mutex<()>>>>>,
}

impl CrateLocks {
    pub fn new() -> Self {
        Self {
            locks: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    pub async fn lock(&self, crate_name: impl Into<String>) -> OwnedMutexGuard<()> {
        let lock = {
            let mut locks = self.locks.lock().await;
            locks
                .entry(crate_name.into())
                .or_insert_with(|| Arc::new(Mutex::new(())))
                .clone()
        };

        lock.lock_owned().await
    }
}
