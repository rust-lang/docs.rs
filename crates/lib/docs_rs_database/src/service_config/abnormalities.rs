use docs_rs_uri::EscapedURI;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Abnormality {
    pub url: EscapedURI,
    pub text: String,
    /// explanation to be shown on the status page, can be HTML
    #[serde(default)]
    pub explanation: Option<String>,
}
