use crate::utils::Dependency;
use derive_more::Deref;
use semver::VersionReq;
use serde::{Deserialize, Serialize};

const DEFAULT_KIND: &str = "normal";

/// The three possible representations of a dependency in our internal JSON format
/// in the `releases.dependencies` column.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
enum Dep {
    Two((String, VersionReq)),
    Three((String, VersionReq, String)),
    Four((String, VersionReq, String, bool)),
}

/// A crate dependency in our internal representation for releases.dependencies json.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Deref)]
#[serde(from = "Dep", into = "Dep")]
pub(crate) struct ReleaseDependency(Dependency);

impl From<Dep> for ReleaseDependency {
    fn from(src: Dep) -> Self {
        let (name, req, kind, optional) = match src {
            Dep::Two((name, req)) => (name, req, DEFAULT_KIND.into(), false),
            Dep::Three((name, req, kind)) => (name, req, kind, false),
            Dep::Four((name, req, kind, optional)) => (name, req, kind, optional),
        };

        ReleaseDependency(Dependency {
            name,
            req,
            kind: Some(kind),
            optional,
            rename: None,
        })
    }
}

impl From<ReleaseDependency> for Dep {
    // dependency serialization for new releases.
    fn from(rd: ReleaseDependency) -> Self {
        let d = rd.0;
        Dep::Four((
            d.name,
            d.req,
            d.kind.unwrap_or_else(|| DEFAULT_KIND.into()),
            d.optional,
        ))
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, Deref)]
#[serde(transparent)]
pub(crate) struct ReleaseDependencyList(Vec<ReleaseDependency>);

impl<I> From<I> for ReleaseDependencyList
where
    I: IntoIterator<Item = Dependency>,
{
    fn from(deps: I) -> Self {
        Self(deps.into_iter().map(ReleaseDependency).collect())
    }
}

impl ReleaseDependencyList {
    pub(crate) fn into_iter_dependencies(self) -> impl Iterator<Item = Dependency> {
        self.0.into_iter().map(|rd| rd.0)
    }
}

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
        let [dep] = deps.0.as_slice() else {
            panic!("expected exactly one dependency");
        };

        assert_eq!(dep.name, "vec_map");
        assert_eq!(dep.req, VersionReq::parse("^0.0.1")?);
        assert_eq!(dep.kind.as_deref(), Some(expected_kind));
        assert_eq!(dep.optional, expected_optional);

        Ok(())
    }
}
