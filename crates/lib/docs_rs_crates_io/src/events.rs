#![allow(clippy::disallowed_types)]

use std::fmt;

/// Identify a kind of change that occurred to a crate
#[derive(Clone, serde::Serialize, serde::Deserialize, Eq, PartialEq, Debug)]
#[serde(tag = "type", content = "payload", rename_all = "snake_case")]
pub enum ChangeV1 {
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

impl ChangeV1 {
    /// Return the added crate, if this is this kind of change.
    pub fn added(&self) -> Option<&CrateVersion> {
        match self {
            ChangeV1::Added(v) => Some(v),
            _ => None,
        }
    }

    /// Return the yanked crate, if this is this kind of change.
    pub fn yanked(&self) -> Option<&CrateVersion> {
        match self {
            ChangeV1::Yanked(v) => Some(v),
            _ => None,
        }
    }

    /// Return the unyanked crate, if this is this kind of change.
    pub fn unyanked(&self) -> Option<&CrateVersion> {
        match self {
            ChangeV1::Unyanked(v) => Some(v),
            _ => None,
        }
    }

    /// Return the deleted crate, if this is this kind of change.
    pub fn crate_deleted(&self) -> Option<&str> {
        match self {
            ChangeV1::CrateDeleted { name, .. } => Some(name.as_str()),
            _ => None,
        }
    }

    /// Return the deleted version crate, if this is this kind of change.
    pub fn version_deleted(&self) -> Option<&CrateVersion> {
        match self {
            ChangeV1::VersionDeleted(v) => Some(v),
            _ => None,
        }
    }
}

impl fmt::Display for ChangeV1 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}",
            match *self {
                ChangeV1::Added(_) => "added",
                ChangeV1::Yanked(_) => "yanked",
                ChangeV1::CrateDeleted { .. } => "crate deleted",
                ChangeV1::VersionDeleted(_) => "version deleted",
                ChangeV1::Unyanked(_) => "unyanked",
            }
        )
    }
}

/// A conventional event envelope for crate index changes.
#[derive(Clone, serde::Serialize, serde::Deserialize, Eq, PartialEq, Debug)]
pub struct Event<T> {
    /// Unique event identifier for deduplication and tracing.
    pub id: String,
    /// Timestamp when the underlying change occurred, as an RFC 3339 string.
    pub occurred_at: String,
    /// System that emitted the event.
    pub source: String,
    /// Version of the serialized event schema.
    pub schema_version: u32,
    /// The typed change payload.
    #[serde(flatten)]
    pub change: T,
}

/// The first version of the public event wire format.
pub type EventV1 = Event<ChangeV1>;

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

    fn event(change: ChangeV1) -> EventV1 {
        EventV1 {
            id: "evt_123".into(),
            occurred_at: "2026-05-22T12:34:56Z".into(),
            source: "crates-index".into(),
            schema_version: 1,
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
                ChangeV1::Added(crate_version.clone()),
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
                ChangeV1::Unyanked(crate_version.clone()),
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
                ChangeV1::Yanked(crate_version.clone()),
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
                ChangeV1::CrateDeleted {
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
                ChangeV1::VersionDeleted(crate_version),
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
        let event = event(ChangeV1::CrateDeleted {
            name: "old-crate".into(),
        });

        assert_eq!(
            serde_json::to_value(&event).unwrap(),
            json!({
                "id": "evt_123",
                "occurred_at": "2026-05-22T12:34:56Z",
                "source": "crates-index",
                "schema_version": 1,
                "type": "crate_deleted",
                "payload": {
                    "name": "old-crate"
                }
            })
        );
    }
}
