use crate::common::{CLIENT, DOCS_RS};
use anyhow::Result;
use docs_rs_types::{KrateName, ReqVersion, Version};
use serde::Deserialize;
use tracing::debug;

#[derive(Debug, Deserialize)]
pub(crate) struct RustdocStatus {
    pub(crate) doc_status: bool,
    pub(crate) version: Version,
}

pub(crate) async fn fetch_rustdoc_status(
    name: &KrateName,
    version: &ReqVersion,
) -> Result<RustdocStatus> {
    debug!("fetching rustdoc status...");
    Ok(CLIENT
        .get(format!("{DOCS_RS}/crate/{name}/{version}/status.json"))
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?)
}
