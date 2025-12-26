use docs_rs_types::{KrateName, Version};

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct QueuedCrate {
    pub(crate) id: i32,
    pub name: KrateName,
    pub version: Version,
    pub priority: i32,
    pub registry: Option<String>,
    pub attempt: i32,
}

#[derive(Debug)]
pub struct BuildPackageSummary {
    pub successful: bool,
    pub should_reattempt: bool,
}

#[cfg(any(test, feature = "testing"))]
impl Default for BuildPackageSummary {
    fn default() -> Self {
        Self {
            successful: true,
            should_reattempt: false,
        }
    }
}
