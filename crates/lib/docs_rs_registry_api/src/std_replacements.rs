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
pub struct ReplacementDetails {
    description: String,
    url: Url,
}

#[cfg(test)]
mod tests {
    use super::*;

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
