use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DocumentationStatistics {
    documentation_file_count: u64,
    last_updated: DateTime<Utc>,
}

impl DocumentationStatistics {
    pub fn documentation_file_count(&self) -> u64 {
        self.documentation_file_count
    }

    pub fn last_updated(&self) -> DateTime<Utc> {
        self.last_updated
    }

    pub fn new(documentation_file_count: u64) -> Self {
        Self {
            documentation_file_count,
            last_updated: Utc::now(),
        }
    }

    #[cfg(any(test, feature = "testing"))]
    pub fn new_at(documentation_file_count: u64, last_updated: DateTime<Utc>) -> Self {
        Self {
            documentation_file_count,
            last_updated,
        }
    }
}
