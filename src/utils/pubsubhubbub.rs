use reqwest::{
    blocking::{Client, Response},
    Result,
};
use std::collections::HashMap;

fn ping_hub(url: &str) -> Result<Response> {
    let mut params = HashMap::with_capacity(2);
    params.insert("hub.mode", "publish");
    params.insert("hub.url", "https://docs.rs/releases/feed");
    let client = Client::new();
    client.post(url).form(&params).send()
}

/// Ping the two predefined hubs. Return either the number of successfully
/// pinged hubs, or the first error.
pub fn ping_hubs() -> Result<usize> {
    vec![
        "https://pubsubhubbub.appspot.com",
        "https://pubsubhubbub.superfeedr.com",
    ]
    .into_iter()
    .map(ping_hub)
    .collect::<Result<Vec<_>>>()
    .map(|v| v.len())
}
