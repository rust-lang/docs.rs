use chrono::{DateTime, Utc};
use std::fmt;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ChangeKind {
    Added,
    AddedAndYanked,
    Unyanked,
    Yanked,
    CrateDeleted,
    VersionDeleted,
}

impl ChangeKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Added => "added",
            Self::AddedAndYanked => "added_and_yanked",
            Self::Unyanked => "unyanked",
            Self::Yanked => "yanked",
            Self::CrateDeleted => "crate_deleted",
            Self::VersionDeleted => "version_deleted",
        }
    }
}

impl fmt::Display for ChangeKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

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
            Self::Added(version) => Some(version),
            _ => None,
        }
    }

    /// Return the yanked crate, if this is this kind of change.
    pub fn yanked(&self) -> Option<&CrateVersion> {
        match self {
            Self::Yanked(version) => Some(version),
            _ => None,
        }
    }

    /// Return the unyanked crate, if this is this kind of change.
    pub fn unyanked(&self) -> Option<&CrateVersion> {
        match self {
            Self::Unyanked(version) => Some(version),
            _ => None,
        }
    }

    /// Return the deleted crate, if this is this kind of change.
    pub fn crate_deleted(&self) -> Option<&str> {
        match self {
            Self::CrateDeleted { name } => Some(name),
            _ => None,
        }
    }

    /// Return the deleted version crate, if this is this kind of change.
    pub fn version_deleted(&self) -> Option<&CrateVersion> {
        match self {
            Self::VersionDeleted(version) => Some(version),
            _ => None,
        }
    }

    pub fn name(&self) -> &str {
        match self {
            Self::Added(crate_version)
            | Self::Unyanked(crate_version)
            | Self::Yanked(crate_version)
            | Self::VersionDeleted(crate_version) => &crate_version.name,
            Self::CrateDeleted { name } => name,
        }
    }

    pub fn version(&self) -> Option<&str> {
        match self {
            Self::Added(crate_version)
            | Self::Unyanked(crate_version)
            | Self::Yanked(crate_version)
            | Self::VersionDeleted(crate_version) => Some(&crate_version.version),
            Self::CrateDeleted { .. } => None,
        }
    }

    pub fn kind(&self) -> ChangeKind {
        match self {
            Self::Added(_) => ChangeKind::Added,
            Self::Unyanked(_) => ChangeKind::Unyanked,
            Self::Yanked(_) => ChangeKind::Yanked,
            Self::CrateDeleted { .. } => ChangeKind::CrateDeleted,
            Self::VersionDeleted(_) => ChangeKind::VersionDeleted,
        }
    }
}

impl fmt::Display for IndexChangeV1 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.kind().fmt(f)
    }
}

/// A conventional event envelope for our events between crates.io & docs.rs
#[derive(Clone, serde::Serialize, serde::Deserialize, Eq, PartialEq, Debug)]
pub struct Event<T> {
    /// Unique event identifier for deduplication and tracing.
    pub id: String,
    /// Timestamp when the event occurred.
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
    /// The semantic version of the crate.
    #[serde(rename = "vers")]
    pub version: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use test_case::test_case;

    fn crate_version() -> CrateVersion {
        CrateVersion {
            name: "clap".into(),
            version: "4.5.0".into(),
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
                "vers": "4.5.0",
            })
        );
    }

    #[test_case(ChangeKind::Added, "added"; "added")]
    #[test_case(ChangeKind::AddedAndYanked, "added_and_yanked"; "added and yanked")]
    #[test_case(ChangeKind::Unyanked, "unyanked"; "unyanked")]
    #[test_case(ChangeKind::Yanked, "yanked"; "yanked")]
    #[test_case(ChangeKind::CrateDeleted, "crate_deleted"; "crate deleted")]
    #[test_case(ChangeKind::VersionDeleted, "version_deleted"; "version deleted")]
    fn change_kind_formats_as_expected(kind: ChangeKind, expected: &str) {
        assert_eq!(kind.as_str(), expected);
        assert_eq!(kind.to_string(), expected);
    }

    #[test_case(IndexChangeV1::Added(crate_version()), "added", json!({ "name": "clap", "vers": "4.5.0" }); "added")]
    #[test_case(IndexChangeV1::Unyanked(crate_version()), "unyanked", json!({ "name": "clap", "vers": "4.5.0" }); "unyanked")]
    #[test_case(IndexChangeV1::Yanked(crate_version()), "yanked", json!({ "name": "clap", "vers": "4.5.0" }); "yanked")]
    #[test_case(IndexChangeV1::CrateDeleted { name: "old-crate".into() }, "crate_deleted", json!({ "name": "old-crate" }); "crate deleted")]
    #[test_case(IndexChangeV1::VersionDeleted(crate_version()), "version_deleted", json!({ "name": "clap", "vers": "4.5.0" }); "version deleted")]
    fn change_serializes_with_expected_variant_shape(
        change: IndexChangeV1,
        change_type: &str,
        payload: serde_json::Value,
    ) {
        assert_eq!(
            serde_json::to_value(change).unwrap(),
            json!({
                "type": change_type,
                "payload": payload,
            })
        );
    }

    #[test_case(IndexChangeV1::Added(crate_version()), ChangeKind::Added, "clap", Some("4.5.0"); "added")]
    #[test_case(IndexChangeV1::Unyanked(crate_version()), ChangeKind::Unyanked, "clap", Some("4.5.0"); "unyanked")]
    #[test_case(IndexChangeV1::Yanked(crate_version()), ChangeKind::Yanked, "clap", Some("4.5.0"); "yanked")]
    #[test_case(IndexChangeV1::CrateDeleted { name: "old-crate".into() }, ChangeKind::CrateDeleted, "old-crate", None; "crate deleted")]
    #[test_case(IndexChangeV1::VersionDeleted(crate_version()), ChangeKind::VersionDeleted, "clap", Some("4.5.0"); "version deleted")]
    fn change_metadata_matches_variant(
        change: IndexChangeV1,
        kind: ChangeKind,
        name: &str,
        version: Option<&str>,
    ) {
        assert_eq!(change.name(), name);
        assert_eq!(change.version(), version);
        assert_eq!(change.kind(), kind);
        assert_eq!(change.to_string(), kind.as_str());
    }

    #[test]
    fn variant_accessors_only_match_their_variant() {
        let added = IndexChangeV1::Added(crate_version());
        assert_eq!(added.added(), Some(&crate_version()));
        assert_eq!(added.yanked(), None);
        assert_eq!(added.unyanked(), None);
        assert_eq!(added.crate_deleted(), None);
        assert_eq!(added.version_deleted(), None);

        assert_eq!(
            IndexChangeV1::Yanked(crate_version()).yanked(),
            Some(&crate_version())
        );
        assert_eq!(
            IndexChangeV1::Unyanked(crate_version()).unyanked(),
            Some(&crate_version())
        );
        assert_eq!(
            IndexChangeV1::CrateDeleted {
                name: "old-crate".into(),
            }
            .crate_deleted(),
            Some("old-crate")
        );
        assert_eq!(
            IndexChangeV1::VersionDeleted(crate_version()).version_deleted(),
            Some(&crate_version())
        );
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
