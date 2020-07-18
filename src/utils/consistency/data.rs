use std::{collections::BTreeMap, fmt::Debug};

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

impl std::fmt::Display for CrateName {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        std::fmt::Display::fmt(&self.0, f)
    }
}

impl std::fmt::Display for Version {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        std::fmt::Display::fmt(&self.0, f)
    }
}
