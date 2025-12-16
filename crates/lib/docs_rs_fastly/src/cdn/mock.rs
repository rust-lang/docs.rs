use crate::CdnBehaviour;
use anyhow::Result;
use docs_rs_headers::{SurrogateKey, SurrogateKeys};
use tokio::sync::Mutex;

#[derive(Debug, Default)]
pub struct MockCdn {
    pub purged: Mutex<SurrogateKeys>,
}

impl CdnBehaviour for MockCdn {
    async fn purge_surrogate_keys<I>(&self, keys: I) -> Result<()>
    where
        I: IntoIterator<Item = SurrogateKey> + 'static + Send,
        I::IntoIter: Send,
    {
        let mut purged = self.purged.lock().await;
        purged.try_extend(keys)?;
        Ok(())
    }
}
