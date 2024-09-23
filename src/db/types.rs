use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, sqlx::Type)]
#[sqlx(type_name = "feature")]
pub struct Feature {
    pub(crate) name: String,
    pub(crate) subfeatures: Vec<String>,
}

impl Feature {
    pub fn new(name: String, subfeatures: Vec<String>) -> Self {
        Feature { name, subfeatures }
    }

    pub fn is_private(&self) -> bool {
        self.name.starts_with('_')
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, sqlx::Type)]
#[sqlx(type_name = "build_status", rename_all = "snake_case")]
#[serde(rename_all = "snake_case")]
pub(crate) enum BuildStatus {
    Success,
    Failure,
    InProgress,
}

impl BuildStatus {
    pub(crate) fn is_success(&self) -> bool {
        matches!(self, BuildStatus::Success)
    }
}

impl<'a> PartialEq<&'a str> for BuildStatus {
    fn eq(&self, other: &&str) -> bool {
        match self {
            Self::Success => *other == "success",
            Self::Failure => *other == "failure",
            Self::InProgress => *other == "in_progress",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use test_case::test_case;

    #[test_case(BuildStatus::Success, "success")]
    #[test_case(BuildStatus::Failure, "failure")]
    #[test_case(BuildStatus::InProgress, "in_progress")]
    fn test_build_status_serialization(status: BuildStatus, expected: &str) {
        let serialized = serde_json::to_string(&status).unwrap();
        assert_eq!(serialized, format!("\"{}\"", expected));
        assert_eq!(
            serde_json::from_str::<BuildStatus>(&serialized).unwrap(),
            status
        );
    }
}
