use postgres_types::{FromSql, ToSql};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, FromSql, ToSql, sqlx::Type)]
#[postgres(name = "feature")]
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

impl sqlx::postgres::PgHasArrayType for Feature {
    fn array_type_info() -> sqlx::postgres::PgTypeInfo {
        sqlx::postgres::PgTypeInfo::with_name("_feature")
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, sqlx::Type)]
#[sqlx(type_name = "build_status", rename_all = "snake_case")]
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
