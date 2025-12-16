use derive_more::{Display, FromStr};
use serde::Serialize;

macro_rules! decl_id {
    ($name:ident, $inner:ty) => {
        #[derive(
            Debug, Clone, Copy, Display, PartialEq, Eq, Hash, FromStr, Serialize, sqlx::Type,
        )]
        #[sqlx(transparent)]
        pub struct $name(pub $inner);
    };
}

decl_id!(CrateId, i32);
decl_id!(ReleaseId, i32);
decl_id!(BuildId, i32);
