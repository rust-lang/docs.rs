use cargo_metadata::{Dependency, DependencyKind};
use semver::VersionReq;
use serde::{Deserialize, Serialize};

/// A subset of `cargo_metadata::Dependency`.
/// Only the data we store in our `releases.dependencies` column.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ReleaseDependency {
    pub(crate) name: String,
    pub(crate) req: VersionReq,
    pub(crate) kind: DependencyKind,
    pub(crate) optional: bool,
}

impl bincode::Encode for ReleaseDependency {
    fn encode<E: bincode::enc::Encoder>(
        &self,
        encoder: &mut E,
    ) -> Result<(), bincode::error::EncodeError> {
        // manual implementation since VersionReq doesn't implement Encode,
        // and I don't want to NewType it right now.
        self.name.encode(encoder)?;
        self.req.to_string().encode(encoder)?;
        self.kind.to_string().encode(encoder)?;
        self.optional.encode(encoder)?;
        Ok(())
    }
}

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
            WithKind((String, VersionReq, DependencyKind)),
            /// [name, version, kind, optional]
            Full((String, VersionReq, DependencyKind, bool)),
        }

        let src = Repr::deserialize(deserializer)?;
        let (name, req, kind, optional) = match src {
            Repr::Basic((name, req)) => (name, req, DependencyKind::default(), false),
            Repr::WithKind((name, req, kind)) => (name, req, kind, false),
            Repr::Full((name, req, kind, optional)) => (name, req, kind, optional),
        };

        Ok(ReleaseDependency {
            name,
            req,
            kind,
            optional,
        })
    }
}

impl Serialize for ReleaseDependency {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        (self.name.as_str(), &self.req, &self.kind, self.optional).serialize(serializer)
    }
}

impl From<Dependency> for ReleaseDependency {
    fn from(dep: Dependency) -> Self {
        ReleaseDependency {
            name: dep.name,
            req: dep.req,
            kind: dep.kind,
            optional: dep.optional,
        }
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

    #[test_case(r#"[["vec_map", "^0.0.1"]]"#, DependencyKind::Normal, false)]
    #[test_case(
        r#"[["vec_map", "^0.0.1", "dev" ]]"#,
        DependencyKind::Development,
        false
    )]
    #[test_case(
        r#"[["vec_map", "^0.0.1", "dev", true ]]"#,
        DependencyKind::Development,
        true
    )]
    fn test_parse_dependency(
        input: &str,
        expected_kind: DependencyKind,
        expected_optional: bool,
    ) -> Result<()> {
        let deps: ReleaseDependencyList = serde_json::from_str(input)?;
        let [dep] = deps.as_slice() else {
            panic!("expected exactly one dependency");
        };

        assert_eq!(dep.name, "vec_map");
        assert_eq!(dep.req, VersionReq::parse("^0.0.1")?);
        assert_eq!(dep.kind, expected_kind);
        assert_eq!(dep.optional, expected_optional);

        Ok(())
    }
}
