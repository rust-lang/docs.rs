#![allow(clippy::disallowed_types)]

use std::fmt;

/// Identify a kind of change that occurred to a crate
#[derive(Clone, serde::Serialize, serde::Deserialize, Eq, PartialEq, Debug)]
#[serde(tag = "type", content = "payload", rename_all = "snake_case")]
pub enum Change {
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

impl Change {
    /// Return the added crate, if this is this kind of change.
    pub fn added(&self) -> Option<&CrateVersion> {
        match self {
            Change::Added(v) => Some(v),
            _ => None,
        }
    }

    /// Return the yanked crate, if this is this kind of change.
    pub fn yanked(&self) -> Option<&CrateVersion> {
        match self {
            Change::Yanked(v) => Some(v),
            _ => None,
        }
    }

    /// Return the unyanked crate, if this is this kind of change.
    pub fn unyanked(&self) -> Option<&CrateVersion> {
        match self {
            Change::Unyanked(v) => Some(v),
            _ => None,
        }
    }

    /// Return the deleted crate, if this is this kind of change.
    pub fn crate_deleted(&self) -> Option<&str> {
        match self {
            Change::CrateDeleted { name, .. } => Some(name.as_str()),
            _ => None,
        }
    }

    /// Return the deleted version crate, if this is this kind of change.
    pub fn version_deleted(&self) -> Option<&CrateVersion> {
        match self {
            Change::VersionDeleted(v) => Some(v),
            _ => None,
        }
    }
}

impl fmt::Display for Change {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}",
            match *self {
                Change::Added(_) => "added",
                Change::Yanked(_) => "yanked",
                Change::CrateDeleted { .. } => "crate deleted",
                Change::VersionDeleted(_) => "version deleted",
                Change::Unyanked(_) => "unyanked",
            }
        )
    }
}

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
                Change::Added(crate_version.clone()),
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
                Change::Unyanked(crate_version.clone()),
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
                Change::Yanked(crate_version.clone()),
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
                Change::CrateDeleted {
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
                Change::VersionDeleted(crate_version),
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
}
