use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::fmt;

#[derive(Debug)]
pub struct CrateData {
    pub owners: Vec<CrateOwner>,
}

#[derive(Debug)]
pub struct ReleaseData {
    pub release_time: DateTime<Utc>,
    pub yanked: bool,
    pub downloads: i32,
}

impl Default for ReleaseData {
    fn default() -> ReleaseData {
        ReleaseData {
            release_time: Utc::now(),
            yanked: false,
            downloads: 0,
        }
    }
}

#[derive(Debug, Clone)]
pub struct CrateOwner {
    pub avatar: String,
    pub login: String,
    pub kind: OwnerKind,
}

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, sqlx::Type,
)]
#[sqlx(type_name = "owner_kind", rename_all = "lowercase")]
#[serde(rename_all = "lowercase")]
pub enum OwnerKind {
    User,
    Team,
}

impl fmt::Display for OwnerKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::User => f.write_str("user"),
            Self::Team => f.write_str("team"),
        }
    }
}

#[derive(Deserialize, Debug, Default)]
#[cfg_attr(test, derive(Serialize))]
pub(crate) struct SearchResponse {
    pub(crate) crates: Option<Vec<SearchCrate>>,
    pub(crate) meta: Option<SearchMeta>,
}

#[derive(Deserialize, Debug)]
#[cfg_attr(test, derive(Serialize, PartialEq, Clone))]
pub struct SearchCrate {
    pub name: String,
}

#[derive(Deserialize, Debug)]
#[cfg_attr(test, derive(Serialize, PartialEq, Clone))]
pub struct SearchMeta {
    pub next_page: Option<String>,
    pub prev_page: Option<String>,
}

#[derive(Deserialize, Debug)]
pub struct Search {
    pub crates: Vec<SearchCrate>,
    pub meta: SearchMeta,
}

#[derive(Deserialize, Debug, Clone, PartialEq, Eq, Default)]
#[cfg_attr(test, derive(Serialize))]
pub struct ApiErrors {
    pub errors: Vec<ApiError>,
}

impl fmt::Display for ApiErrors {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for error in &self.errors {
            writeln!(f, "{}", error)?;
        }
        Ok(())
    }
}

#[derive(Deserialize, Debug, Clone, PartialEq, Eq)]
#[cfg_attr(test, derive(Serialize))]
pub struct ApiError {
    pub detail: Option<String>,
}

impl fmt::Display for ApiError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}",
            self.detail.as_deref().unwrap_or("Unknown API Error")
        )
    }
}
