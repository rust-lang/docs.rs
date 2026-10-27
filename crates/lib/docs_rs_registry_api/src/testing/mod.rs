mod test_env;

use docs_rs_types::{KrateName, Version};
pub use test_env::TestRegistry;

pub fn registry_entry(
    name: &KrateName,
    version: &Version,
    pubtime: Option<&str>,
    yanked: bool,
) -> serde_json::Value {
    serde_json::json!({
        "name": name.to_string(),
        "vers": version.to_string(),
        "pubtime": pubtime,
        "deps": [],
        "cksum": "0".repeat(64),
        "features": {},
        "yanked": yanked,
    })
}
