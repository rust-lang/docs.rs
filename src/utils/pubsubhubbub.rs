use futures_util::stream::{self, TryStreamExt};
use reqwest::{Client, Result};
use std::{collections::HashMap, sync::Arc};

/// The maximum of concurrent pings, `None` for no limit
const MAX_CONCURRENT_PINGS: Option<usize> = None;
const HUBS: &[&str] = &[
    "https://pubsubhubbub.appspot.com",
    "https://pubsubhubbub.superfeedr.com",
];

#[derive(Debug)]
pub struct HubPinger {
    params: Arc<HashMap<&'static str, &'static str>>,
    client: Arc<Client>,
}

impl HubPinger {
    pub fn new() -> Self {
        let client = Arc::new(Client::new());
        let params = {
            let mut params = HashMap::with_capacity(2);
            params.insert("hub.mode", "publish");
            params.insert("hub.url", "https://docs.rs/releases/feed");

            Arc::new(params)
        };

        Self { params, client }
    }

    /// Ping the predefined hubs. Return either the number of successfully pinged hubs or the first error.
    pub async fn ping_hubs(&self) -> Result<usize> {
        stream::iter(HUBS.iter().map(Ok))
            .try_for_each_concurrent(MAX_CONCURRENT_PINGS, |&url| {
                let (client, params) = (Arc::clone(&self.client), Arc::clone(&self.params));
                async move { client.post(url).form(&*params).send().await.map(drop) }
            })
            .await?;

        Ok(HUBS.len())
    }
}
