use postgres_types::{FromSql, ToSql};
use serde::Serialize;

#[derive(Debug, Clone, PartialEq, Serialize, FromSql, ToSql)]
#[postgres(name = "feature")]
pub struct Feature {
    name: String,
    subfeatures: Vec<String>,
}

impl Feature {
    pub fn new(name: String, subfeatures: Vec<String>) -> Self {
        Feature { name, subfeatures }
    }
}
