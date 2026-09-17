use docs_rs_types::KrateName;
use serde::Deserialize;
use std::{
    collections::HashMap,
    sync::{Arc, LazyLock},
    time::Duration,
};
use url::Url;

pub(crate) const CACHE_TTL: Duration = Duration::from_hours(1);

pub(crate) const FETCH_URL: LazyLock<Url> = LazyLock::new(|| {
    Url::parse("https://rust-lang.github.io/std-replacement-data/all.json").unwrap()
});

pub type StdReplacements = HashMap<KrateName, Arc<ReplacementDetails>>;

#[derive(Debug, Deserialize)]
#[cfg_attr(any(test, feature = "testing"), derive(serde::Serialize))]
pub struct ReplacementDetails {
    description: String,
    url: Url,
}

#[cfg(test)]
mod tests {
    use super::*;
    use test_case::test_case;

    #[test]
    fn test_parse_empty_list() -> anyhow::Result<()> {
        let parsed: StdReplacements = serde_json::from_str("{}")?;
        assert!(parsed.is_empty());
        Ok(())
    }

    #[test_case(serde_json::json!({"url": "https://example.com"}); "missing description")]
    #[test_case(serde_json::json!({"description": "replacement"}); "missing url")]
    #[test_case(serde_json::json!({"description": "replacement", "url": "not a url"}); "invalid url")]
    fn test_parse_invalid_details(details: serde_json::Value) {
        assert!(
            serde_json::from_value::<StdReplacements>(serde_json::json!({
                "void": details,
            }))
            .is_err()
        );
    }

    #[test]
    fn test_parse_invalid_crate_name() {
        assert!(
            serde_json::from_value::<StdReplacements>(serde_json::json!({
                "invalid crate name": {
                    "description": "replacement",
                    "url": "https://example.com",
                },
            }))
            .is_err()
        );
    }

    #[test]
    fn test_parse_list() -> anyhow::Result<()> {
        let data = serde_json::json!({
          "array-init": {
            "description": "array-init-desc",
            "url": "https://array-init-url.de"
          },
          "void": {
            "description": "void-desc",
            "url": "https://void-url.de"
          }
        });

        let parsed: StdReplacements = serde_json::from_value(data)?;

        assert_eq!(parsed.len(), 2);

        let array_init = &parsed[&KrateName::from_static("array-init")];
        assert_eq!(array_init.description, "array-init-desc");
        assert_eq!(array_init.url.as_str(), "https://array-init-url.de/");

        let void = &parsed[&KrateName::from_static("void")];
        assert_eq!(void.description, "void-desc");
        assert_eq!(void.url.as_str(), "https://void-url.de/");

        Ok(())
    }
}
