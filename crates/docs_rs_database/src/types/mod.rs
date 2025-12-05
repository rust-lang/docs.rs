use derive_more::{Display, FromStr};
use serde::Serialize;

pub mod version;

#[derive(Debug, Clone, Copy, Display, PartialEq, Eq, Hash, Serialize, sqlx::Type)]
#[sqlx(transparent)]
pub struct CrateId(pub i32);

#[derive(Debug, Clone, Copy, Display, PartialEq, Eq, Hash, FromStr, Serialize, sqlx::Type)]
#[sqlx(transparent)]
pub struct ReleaseId(pub i32);

#[derive(Debug, Clone, Copy, Display, PartialEq, Eq, Hash, Serialize, sqlx::Type)]
#[sqlx(transparent)]
pub struct BuildId(pub i32);
