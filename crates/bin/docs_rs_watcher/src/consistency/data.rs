use docs_rs_types::{KrateName, Version};

#[derive(Clone, PartialEq, Debug)]
pub(super) struct Crate {
    pub(super) name: KrateName,
    pub(super) releases: Releases,
}

pub(super) type Crates = Vec<Crate>;

pub(super) type Releases = Vec<Release>;

#[derive(Clone, Debug, PartialEq)]
pub(super) struct Release {
    pub(super) version: Version,
    pub(super) yanked: Option<bool>,
}
