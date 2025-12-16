use serde::Serialize;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, sqlx::Type)]
#[sqlx(type_name = "feature")]
pub struct Feature {
    pub name: String,
    pub subfeatures: Vec<String>,
}

impl Feature {
    pub fn new(name: String, subfeatures: Vec<String>) -> Self {
        Feature { name, subfeatures }
    }

    pub fn is_private(&self) -> bool {
        self.name.starts_with('_')
    }
}
