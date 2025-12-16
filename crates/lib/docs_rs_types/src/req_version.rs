use crate::version::Version;
use semver::VersionReq;
use serde_with::{DeserializeFromStr, SerializeDisplay};
use std::{
    fmt::{self, Display},
    str::FromStr,
};

/// Represents a version identifier in a request in the original state.
/// Can be an exact version, a semver requirement, or the string "latest".
#[derive(Debug, Default, Clone, PartialEq, Eq, SerializeDisplay, DeserializeFromStr)]
pub enum ReqVersion {
    Exact(Version),
    Semver(VersionReq),
    #[default]
    Latest,
}

impl ReqVersion {
    pub fn is_latest(&self) -> bool {
        matches!(self, ReqVersion::Latest)
    }
}

impl bincode::Encode for ReqVersion {
    fn encode<E: bincode::enc::Encoder>(
        &self,
        encoder: &mut E,
    ) -> Result<(), bincode::error::EncodeError> {
        // manual implementation since VersionReq doesn't implement Encode,
        // and I don't want to NewType it right now.
        match self {
            ReqVersion::Exact(v) => {
                0u8.encode(encoder)?;
                v.encode(encoder)
            }
            ReqVersion::Semver(req) => {
                1u8.encode(encoder)?;
                req.to_string().encode(encoder)
            }
            ReqVersion::Latest => {
                2u8.encode(encoder)?;
                Ok(())
            }
        }
    }
}

impl Display for ReqVersion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ReqVersion::Exact(version) => version.fmt(f),
            ReqVersion::Semver(version_req) => version_req.fmt(f),
            ReqVersion::Latest => write!(f, "latest"),
        }
    }
}

impl FromStr for ReqVersion {
    type Err = semver::Error;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if s == "latest" {
            Ok(ReqVersion::Latest)
        } else if let Ok(version) = Version::parse(s) {
            Ok(ReqVersion::Exact(version))
        } else if s.is_empty() || s == "newest" {
            Ok(ReqVersion::Semver(VersionReq::STAR))
        } else {
            VersionReq::parse(s).map(ReqVersion::Semver)
        }
    }
}

impl From<&ReqVersion> for ReqVersion {
    fn from(value: &ReqVersion) -> Self {
        value.clone()
    }
}

impl From<Version> for ReqVersion {
    fn from(value: Version) -> Self {
        ReqVersion::Exact(value)
    }
}

impl From<&Version> for ReqVersion {
    fn from(value: &Version) -> Self {
        value.clone().into()
    }
}

impl From<VersionReq> for ReqVersion {
    fn from(value: VersionReq) -> Self {
        ReqVersion::Semver(value)
    }
}

impl From<&VersionReq> for ReqVersion {
    fn from(value: &VersionReq) -> Self {
        value.clone().into()
    }
}

impl TryFrom<String> for ReqVersion {
    type Error = semver::Error;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        value.parse()
    }
}

impl TryFrom<&str> for ReqVersion {
    type Error = semver::Error;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        value.parse()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use test_case::test_case;

    #[test]
    fn test_parse_req_version_latest() {
        let req_version: ReqVersion = "latest".parse().unwrap();
        assert_eq!(req_version, ReqVersion::Latest);
        assert_eq!(req_version.to_string(), "latest");
    }

    #[test_case("1.2.3")]
    fn test_parse_req_version_exact(input: &str) {
        let req_version: ReqVersion = input.parse().unwrap();
        assert_eq!(
            req_version,
            ReqVersion::Exact(Version::parse(input).unwrap())
        );
        assert_eq!(req_version.to_string(), input);
    }

    #[test_case("^1.2.3")]
    #[test_case("*")]
    fn test_parse_req_version_semver(input: &str) {
        let req_version: ReqVersion = input.parse().unwrap();
        assert_eq!(
            req_version,
            ReqVersion::Semver(VersionReq::parse(input).unwrap())
        );
        assert_eq!(req_version.to_string(), input);
    }

    #[test_case("")]
    #[test_case("newest")]
    fn test_parse_req_version_semver_latest(input: &str) {
        let req_version: ReqVersion = input.parse().unwrap();
        assert_eq!(req_version, ReqVersion::Semver(VersionReq::STAR));
        assert_eq!(req_version.to_string(), "*")
    }
}
