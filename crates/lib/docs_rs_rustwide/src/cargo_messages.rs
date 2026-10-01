use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Deserialize, Serialize)]
#[serde(transparent)]
pub struct CargoMessage(serde_json::Value);
// TODO: nicer debug impl
//
impl CargoMessage {
    pub fn reason(&self) -> Option<&str> {
        self.0.get("reason").and_then(Value::as_str)
    }

    pub fn rendered(&self) -> Option<&str> {
        self.0
            .pointer("/message/rendered")
            .and_then(serde_json::Value::as_str)
    }
}

pub type CargoMessages = Vec<CargoMessage>;
