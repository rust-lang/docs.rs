use super::Dependency;
use derive_more::Deref;
use semver::VersionReq;
use serde::{Deserialize, Serialize};

const DEFAULT_KIND: &str = "normal";

/// A crate dependency in our internal representation for releases.dependencies json.
#[derive(Debug, Clone, PartialEq, Deref)]
pub(crate) struct ReleaseDependency(Dependency);

impl<'de> Deserialize<'de> for ReleaseDependency {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        /// The three possible representations of a dependency in our internal JSON format
        /// in the `releases.dependencies` column.
        #[derive(Serialize, Deserialize)]
        #[serde(untagged)]
        enum Repr {
            /// just [name, version]``
            Basic((String, VersionReq)),
            /// [name, version, kind]
            WithKind((String, VersionReq, String)),
            /// [name, version, kind, optional]
            Full((String, VersionReq, String, bool)),
        }

        let src = Repr::deserialize(deserializer)?;
        let (name, req, kind, optional) = match src {
            Repr::Basic((name, req)) => (name, req, DEFAULT_KIND.into(), false),
            Repr::WithKind((name, req, kind)) => (name, req, kind, false),
            Repr::Full((name, req, kind, optional)) => (name, req, kind, optional),
        };

        Ok(ReleaseDependency(Dependency {
            name,
            req,
            kind: Some(kind),
            optional,
            rename: None,
        }))
    }
}

impl Serialize for ReleaseDependency {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        let dep = &self.0;
        let kind = dep.kind.as_deref().unwrap_or(DEFAULT_KIND);
        (dep.name.as_str(), &dep.req, kind, dep.optional).serialize(serializer)
    }
}

impl From<Dependency> for ReleaseDependency {
    fn from(dep: Dependency) -> Self {
        ReleaseDependency(dep)
    }
}

impl From<ReleaseDependency> for Dependency {
    fn from(dep: ReleaseDependency) -> Self {
        dep.0
    }
}

pub(crate) type ReleaseDependencyList = Vec<ReleaseDependency>;

#[cfg(test)]
mod tests {
    use super::*;
    use anyhow::Result;
    use test_case::test_case;

    #[test_case("[]", "[]"; "empty")]
    #[test_case(
        r#"[["vec_map", "^0.0.1"]]"#,
        r#"[["vec_map","^0.0.1","normal",false]]"#;
        "2-tuple"
    )]
    #[test_case(
        r#"[["vec_map", "^0.0.1", "normal" ]]"#,
        r#"[["vec_map","^0.0.1","normal",false]]"#;
        "3-tuple"
    )]
    #[test_case(
        r#"[["rand", "^0.9", "normal", false], ["sdl3", "^0.16", "normal", false]]"#,
        r#"[["rand","^0.9","normal",false],["sdl3","^0.16","normal",false]]"#;
        "4-tuple"
    )]
    #[test_case(
        r#"[["byteorder", "^0.5", "normal", false],["clippy", "^0", "normal", true]]"#,
        r#"[["byteorder","^0.5","normal",false],["clippy","^0","normal",true]]"#;
        "with optional"
    )]
    fn test_parse_release_dependency_json(input: &str, output: &str) -> Result<()> {
        let deps: ReleaseDependencyList = serde_json::from_str(input)?;

        assert_eq!(serde_json::to_string(&deps)?, output);
        Ok(())
    }

    #[test_case(r#"[["vec_map", "^0.0.1"]]"#, "normal", false)]
    #[test_case(r#"[["vec_map", "^0.0.1", "dev" ]]"#, "dev", false)]
    #[test_case(r#"[["vec_map", "^0.0.1", "dev", true ]]"#, "dev", true)]
    fn test_parse_dependency(
        input: &str,
        expected_kind: &str,
        expected_optional: bool,
    ) -> Result<()> {
        let deps: ReleaseDependencyList = serde_json::from_str(input)?;
        let [dep] = deps.as_slice() else {
            panic!("expected exactly one dependency");
        };

        assert_eq!(dep.name, "vec_map");
        assert_eq!(dep.req, VersionReq::parse("^0.0.1")?);
        assert_eq!(dep.kind.as_deref(), Some(expected_kind));
        assert_eq!(dep.optional, expected_optional);

        Ok(())
    }
}
