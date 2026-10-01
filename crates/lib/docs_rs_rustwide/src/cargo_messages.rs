use rustwide::cmd::ProcessLinesActions;
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

/// Retain whole Cargo JSONL records up to the same byte limit as the step log.
pub(crate) struct CargoMessageCollector {
    messages: CargoMessages,
    retained_bytes: usize,
    max_bytes: usize,
}

impl CargoMessageCollector {
    pub(crate) fn new(max_bytes: usize) -> Self {
        Self {
            messages: Vec::new(),
            retained_bytes: 0,
            max_bytes,
        }
    }

    pub(crate) fn process_line(&mut self, line: &str, actions: &mut ProcessLinesActions) {
        let Ok(message) = serde_json::from_str::<CargoMessage>(line) else {
            return;
        };
        let Some(reason) = message.reason() else {
            return;
        };

        if reason == "compiler-message" {
            if let Some(rendered) = message.rendered() {
                actions.replace_with_lines(rendered.lines());
            } else {
                actions.remove_line();
            }
            self.push(line, message);
        } else {
            // Cargo protocol records are not user output. Removing them keeps the captured build
            // log readable and leaves non-JSON process output untouched.
            actions.remove_line();
        }
    }

    fn push(&mut self, line: &str, message: CargoMessage) {
        let record_bytes = line.len().saturating_add(1);
        if record_bytes <= self.max_bytes.saturating_sub(self.retained_bytes) {
            self.messages.push(message);
            self.retained_bytes += record_bytes;
        }
    }

    pub(crate) fn into_messages(self) -> CargoMessages {
        self.messages
    }
}
