use reqwest::{Client, Response, Result};
use std::collections::HashMap;

const PING_HUBS: &[&str] = &[
    "https://pubsubhubbub.appspot.com",
    "https://pubsubhubbub.superfeedr.com",
];

async fn ping_hub(url: &str) -> Result<Response> {
    let mut params = HashMap::with_capacity(2);
    params.insert("hub.mode", "publish");
    params.insert("hub.url", "https://docs.rs/releases/feed");

    let client = Client::new();
    client.post(url).form(&params).send().await
}

/// Ping the predefined hubs. Return either the number of successfully pinged hubs or the first error.
pub async fn ping_hubs() -> Result<usize> {
    for hub in PING_HUBS {
        ping_hub(hub).await?;
    }

    Ok(PING_HUBS.len())
}
