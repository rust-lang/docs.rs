#![allow(clippy::disallowed_types)]

use chrono::{DateTime, Utc};
use std::fmt;

/// A change that can happen to a crate on our index.
#[derive(Clone, serde::Serialize, serde::Deserialize, Eq, PartialEq, Debug)]
#[serde(tag = "type", content = "payload", rename_all = "snake_case")]
pub enum IndexChangeV1 {
    /// A crate version was added.
    Added(CrateVersion),
    /// A crate version was unyanked.
    Unyanked(CrateVersion),
    /// A crate version was yanked.
    Yanked(CrateVersion),
    /// The name of the crate whose file was deleted, which implies all versions were deleted as well.
    CrateDeleted { name: String },
    /// A crate version was deleted.
    VersionDeleted(CrateVersion),
}

impl IndexChangeV1 {
    /// Return the added crate, if this is this kind of change.
    pub fn added(&self) -> Option<&CrateVersion> {
        match self {
            IndexChangeV1::Added(v) => Some(v),
            _ => None,
        }
    }

    /// Return the yanked crate, if this is this kind of change.
    pub fn yanked(&self) -> Option<&CrateVersion> {
        match self {
            IndexChangeV1::Yanked(v) => Some(v),
            _ => None,
        }
    }

    /// Return the unyanked crate, if this is this kind of change.
    pub fn unyanked(&self) -> Option<&CrateVersion> {
        match self {
            IndexChangeV1::Unyanked(v) => Some(v),
            _ => None,
        }
    }

    /// Return the deleted crate, if this is this kind of change.
    pub fn crate_deleted(&self) -> Option<&str> {
        match self {
            IndexChangeV1::CrateDeleted { name } => Some(name.as_str()),
            _ => None,
        }
    }

    /// Return the deleted version crate, if this is this kind of change.
    pub fn version_deleted(&self) -> Option<&CrateVersion> {
        match self {
            IndexChangeV1::VersionDeleted(v) => Some(v),
            _ => None,
        }
    }

    pub fn name(&self) -> &str {
        match self {
            IndexChangeV1::Added(crate_version) => &crate_version.name,
            IndexChangeV1::Unyanked(crate_version) => &crate_version.name,
            IndexChangeV1::Yanked(crate_version) => &crate_version.name,
            IndexChangeV1::CrateDeleted { name } => &name,
            IndexChangeV1::VersionDeleted(crate_version) => &crate_version.name,
        }
    }
}

impl fmt::Display for IndexChangeV1 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}",
            match *self {
                IndexChangeV1::Added(_) => "added",
                IndexChangeV1::Yanked(_) => "yanked",
                IndexChangeV1::CrateDeleted { .. } => "crate deleted",
                IndexChangeV1::VersionDeleted(_) => "version deleted",
                IndexChangeV1::Unyanked(_) => "unyanked",
            }
        )
    }
}

/// A conventional event envelope for our events between crates.io & docs.rs
#[derive(Clone, serde::Serialize, serde::Deserialize, Eq, PartialEq, Debug)]
pub struct Event<T> {
    /// Unique event identifier for deduplication and tracing.
    pub id: String,
    /// Timestamp when the event occured
    pub occurred_at: DateTime<Utc>,
    /// The typed payload.
    #[serde(flatten)]
    pub change: T,
}

/// The first version of the public event wire format.
pub type IndexChangeEventV1 = Event<IndexChangeV1>;

/// Pack all information we know about a change made to a version of a crate.
#[derive(Clone, serde::Serialize, serde::Deserialize, Eq, PartialEq, Debug)]
pub struct CrateVersion {
    /// The crate name, i.e. `clap`.
    pub name: String,
    /// is the release yanked?
    pub yanked: bool,
    /// The semantic version of the crate.
    #[serde(rename = "vers")]
    pub version: semver::Version,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn crate_version() -> CrateVersion {
        CrateVersion {
            name: "clap".into(),
            yanked: false,
            version: semver::Version::new(4, 5, 0),
        }
    }

    fn event(change: IndexChangeV1) -> IndexChangeEventV1 {
        IndexChangeEventV1 {
            id: "evt_123".into(),
            occurred_at: DateTime::parse_from_rfc3339("2026-05-22T12:34:56Z")
                .unwrap()
                .with_timezone(&Utc),
            change,
        }
    }

    #[test]
    fn crate_version_serializes_with_vers_field() {
        let event = crate_version();

        assert_eq!(
            serde_json::to_value(&event).unwrap(),
            json!({
                "name": "clap",
                "yanked": false,
                "vers": "4.5.0",
            })
        );
    }

    #[test]
    fn change_serializes_with_expected_variant_shapes() {
        let crate_version = crate_version();

        let cases = [
            (
                IndexChangeV1::Added(crate_version.clone()),
                json!({
                    "type": "added",
                    "payload": {
                        "name": "clap",
                        "yanked": false,
                        "vers": "4.5.0",
                    }
                }),
            ),
            (
                IndexChangeV1::Unyanked(crate_version.clone()),
                json!({
                    "type": "unyanked",
                    "payload": {
                        "name": "clap",
                        "yanked": false,
                        "vers": "4.5.0",
                    }
                }),
            ),
            (
                IndexChangeV1::Yanked(crate_version.clone()),
                json!({
                    "type": "yanked",
                    "payload": {
                        "name": "clap",
                        "yanked": false,
                        "vers": "4.5.0",
                    }
                }),
            ),
            (
                IndexChangeV1::CrateDeleted {
                    name: "old-crate".into(),
                },
                json!({
                    "type": "crate_deleted",
                    "payload": {
                        "name": "old-crate"
                    }
                }),
            ),
            (
                IndexChangeV1::VersionDeleted(crate_version),
                json!({
                    "type": "version_deleted",
                    "payload": {
                        "name": "clap",
                        "yanked": false,
                        "vers": "4.5.0",
                    }
                }),
            ),
        ];

        for (event, expected) in cases {
            assert_eq!(serde_json::to_value(&event).unwrap(), expected);
        }
    }

    #[test]
    fn event_serializes_with_minimum_metadata() {
        let event = event(IndexChangeV1::CrateDeleted {
            name: "old-crate".into(),
        });

        assert_eq!(
            serde_json::to_value(&event).unwrap(),
            json!({
                "id": "evt_123",
                "occurred_at": "2026-05-22T12:34:56Z",
                "type": "crate_deleted",
                "payload": {
                    "name": "old-crate"
                }
            })
        );
    }

    #[test]
    fn event_deserializes_rfc3339_occurred_at() {
        let event: IndexChangeEventV1 = serde_json::from_value(json!({
            "id": "evt_123",
            "occurred_at": "2026-05-22T12:34:56Z",
            "type": "crate_deleted",
            "payload": {
                "name": "old-crate"
            }
        }))
        .unwrap();

        assert_eq!(
            event.occurred_at,
            DateTime::parse_from_rfc3339("2026-05-22T12:34:56Z")
                .unwrap()
                .with_timezone(&Utc)
        );
    }
}
