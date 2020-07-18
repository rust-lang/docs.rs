use std::{
    cmp::PartialEq,
    collections::BTreeMap,
    fmt::{self, Debug, Display, Formatter},
};

#[derive(Default, Debug)]
pub(crate) struct Data {
    pub(crate) crates: BTreeMap<CrateName, Crate>,
}

#[derive(PartialOrd, Ord, PartialEq, Eq, Clone, Default, Debug)]
pub(crate) struct CrateName(pub(crate) String);

#[derive(Default, Debug)]
pub(crate) struct Crate {
    pub(crate) releases: BTreeMap<Version, Release>,
}

#[derive(PartialOrd, Ord, PartialEq, Eq, Clone, Default, Debug)]
pub(crate) struct Version(pub(crate) String);

#[derive(Default, Debug)]
pub(crate) struct Release {}

impl PartialEq<String> for CrateName {
    fn eq(&self, other: &String) -> bool {
        self.0 == *other
    }
}

impl PartialEq<String> for Version {
    fn eq(&self, other: &String) -> bool {
        self.0 == *other
    }
}

impl Display for CrateName {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        Display::fmt(&self.0, f)
    }
}

impl Display for Version {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        Display::fmt(&self.0, f)
    }
}
