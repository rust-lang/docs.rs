use rustwide::cmd::ProcessLinesActions;
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Deserialize, Serialize)]
#[serde(transparent)]
pub struct RawCargoMessage(serde_json::Value);

impl RawCargoMessage {
    fn is_build_started(&self) -> bool {
        self.reason()
            .is_some_and(|reason| reason == "build-started")
            && self.0.get("run_id").is_some()
    }

    fn is_build_finished(&self) -> bool {
        self.reason()
            .is_some_and(|reason| reason == "build-finished")
            && self.0.get("success").is_some()
    }

    fn is_build_script_executed(&self) -> bool {
        self.reason()
            .is_some_and(|reason| reason == "build-script-executed")
            && self.0.get("package_id").is_some()
            && self.0.get("linked_libs").is_some()
    }

    fn is_compiler_artifact(&self) -> bool {
        self.reason()
            .is_some_and(|reason| reason == "compiler-artifact")
            && self.0.get("package_id").is_some()
            && self.0.get("manifest_path").is_some()
    }

    fn is_compiler_message(&self) -> bool {
        self.reason()
            .is_some_and(|reason| reason == "compiler-message")
            && self.0.get("package_id").is_some()
            && self.0.get("manifest_path").is_some()
    }

    pub fn reason(&self) -> Option<&str> {
        self.0.get("reason").and_then(Value::as_str)
    }

    pub fn rendered(&self) -> Option<&str> {
        self.0
            .pointer("/message/rendered")
            .and_then(serde_json::Value::as_str)
    }
}

pub type RawCargoMessages = Vec<RawCargoMessage>;

/// Retain whole Cargo JSONL records up to the same byte limit as the step log.
pub(crate) struct CargoMessageCollector {
    messages: RawCargoMessages,
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
        let line = line.trim();
        if !(line.starts_with('{') && line.ends_with('}')) {
            return;
        }

        let Ok(message) = serde_json::from_str::<RawCargoMessage>(line) else {
            return;
        };

        if message.is_compiler_artifact()
            || message.is_build_finished()
            || message.is_build_script_executed()
            || message.is_build_started()
        {
            actions.remove_line();
        }

        if message.is_compiler_message() {
            if let Some(rendered) = message.rendered() {
                // if we have the rendered version in the json, replace the json log line
                // with the rendered version.
                actions.replace_with_lines(rendered.lines());
            } else {
                // compiler-messages without rendering shouldn't happen?
                // just to be safe, we don't drop it and leave it in the logs, but still
                // add it to our cargo messages.
            }
            self.push(line, message);
        }

        // Other JSON lines are kept, as we don't know what they are.
    }

    fn push(&mut self, line: &str, message: RawCargoMessage) {
        let record_bytes = line.len().saturating_add(1);
        if record_bytes <= self.max_bytes.saturating_sub(self.retained_bytes) {
            self.messages.push(message);
            self.retained_bytes += record_bytes;
        }
    }

    pub(crate) fn into_messages(self) -> RawCargoMessages {
        self.messages
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn collection_respects_the_log_size_limit() {
        let line = r#" { "reason": "compiler-message" } "#;
        let message = serde_json::from_str(line).unwrap();
        let mut collector = CargoMessageCollector::new(line.len() + 1);

        collector.push(line, message);
        collector.push(line, serde_json::from_str(line).unwrap());

        assert_eq!(collector.into_messages().len(), 1);
    }

    #[test]
    fn non_cargo_json_is_not_a_compiler_message() {
        let message =
            serde_json::from_str::<RawCargoMessage>(r#"{"source":"build-script"}"#).unwrap();

        assert!(!message.is_compiler_message());
    }
}
