use postgres_types::{FromSql, ToSql};
use serde::Serialize;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, FromSql, ToSql)]
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
