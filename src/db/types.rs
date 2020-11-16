use postgres_types::{FromSql, ToSql};
use serde::Serialize;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, FromSql, ToSql)]
#[postgres(name = "feature")]
pub struct Feature {
    pub(crate) name: String,
    pub(crate) subfeatures: Vec<String>,
    pub(crate) optional_dependency: Option<bool>,
}

impl Feature {
    pub fn new(name: String, subfeatures: Vec<String>, optional_dependency: bool) -> Self {
        Feature {
            name,
            subfeatures,
            optional_dependency: Some(optional_dependency),
        }
    }

    pub fn is_private(&self) -> bool {
        self.name.starts_with('_')
    }
}
