use serde::Serialize;
use std::{fmt, str::FromStr};

macro_rules! decl_id {
    ($name:ident, $inner:ty) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, sqlx::Type)]
        #[sqlx(transparent)]
        pub struct $name(pub $inner);

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, "{}", self.0)
            }
        }

        impl FromStr for $name {
            type Err = <$inner as FromStr>::Err;

            fn from_str(s: &str) -> Result<Self, Self::Err> {
                let inner = s.parse::<$inner>()?;
                Ok($name(inner))
            }
        }
    };
}

decl_id!(CrateId, i32);
decl_id!(ReleaseId, i32);
decl_id!(BuildId, i32);
