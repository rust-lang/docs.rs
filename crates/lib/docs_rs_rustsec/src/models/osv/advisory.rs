// Adapted from RustSec (MIT); see ../LICENSE-MIT.
//! OSV advisory models used by the HTTP client.
use crate::models::advisory::{Id, Informational};
use serde::{Deserialize, Serialize};

/// Security advisory in the format defined by <https://github.com/google/osv>
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OsvAdvisory {
    id: Id,
    #[serde(skip_serializing_if = "Option::is_none")]
    withdrawn: Option<String>, // maybe add an rfc3339 newtype?
    summary: String,
    #[serde(default)]
    affected: Vec<OsvAffected>,
}

/// A package affected by an OSV advisory, including RustSec-specific metadata.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OsvAffected {
    database_specific: OsvDatabaseSpecific,
    ranges: Option<Vec<OsvJsonRange>>,
}

impl OsvAffected {
    /// RustSec informational classification for this package, if any.
    pub fn informational(&self) -> Option<&Informational> {
        self.database_specific.informational.as_ref()
    }

    /// Whether any affected range includes a patched version.
    pub fn has_patched_versions(&self) -> bool {
        self.ranges.iter().flatten().any(|range| {
            range
                .events
                .iter()
                .any(|event| matches!(event, OsvTimelineEvent::Fixed(_)))
        })
    }
}

/// An OSV affected range with an ordered sequence of version events.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OsvJsonRange {
    events: Vec<OsvTimelineEvent>,
}

/// A version marking a boundary of an affected range.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum OsvTimelineEvent {
    /// First affected version; `0` denotes all earlier versions.
    #[serde(rename = "introduced")]
    Introduced(String),
    /// First version containing a fix (excluded from the affected range).
    #[serde(rename = "fixed")]
    Fixed(String),
    /// Last affected version (included in the affected range).
    #[serde(rename = "last_affected")]
    LastAffected(String),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct OsvDatabaseSpecific {
    informational: Option<Informational>,
}

impl OsvAdvisory {
    /// A short summary of the advisory.
    pub fn summary(&self) -> &str {
        &self.summary
    }

    /// Affected packages and their RustSec-specific metadata.
    pub fn affected(&self) -> &[OsvAffected] {
        &self.affected
    }

    /// Advisory ID.
    pub fn id(&self) -> &Id {
        &self.id
    }

    /// Whether this advisory has been withdrawn.
    pub fn withdrawn(&self) -> bool {
        self.withdrawn.is_some()
    }
}
