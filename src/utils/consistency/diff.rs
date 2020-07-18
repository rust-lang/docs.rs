use super::data::{Crate, CrateName, Data, Release, Version};
use std::{
    cmp::Ordering,
    collections::{btree_map::IntoIter, BTreeMap},
    fmt::Debug,
    iter::Peekable,
};

#[derive(Debug)]
pub(crate) struct DataDiff {
    pub(crate) crates: DiffMap<CrateName, Crate>,
}

#[derive(Debug)]
pub(crate) struct CrateDiff {
    pub(crate) releases: DiffMap<Version, Release>,
}

#[derive(Debug)]
pub(crate) struct ReleaseDiff {}

pub(crate) enum Diff<Key, Value: Diffable> {
    Both(Key, Value::Diff),
    Left(Key, Value),
    Right(Key, Value),
}

pub(crate) trait Diffable {
    type Diff;

    fn diff(self, other: Self) -> Self::Diff;
}

#[derive(Debug)]
pub(crate) struct DiffMap<Key, Value> {
    left: Peekable<std::collections::btree_map::IntoIter<Key, Value>>,
    right: Peekable<IntoIter<Key, Value>>,
}

impl<Key, Value> DiffMap<Key, Value> {
    fn new(left: BTreeMap<Key, Value>, right: BTreeMap<Key, Value>) -> Self {
        Self {
            left: left.into_iter().peekable(),
            right: right.into_iter().peekable(),
        }
    }
}

impl<Key: Ord, Value: Diffable> Iterator for DiffMap<Key, Value> {
    type Item = Diff<Key, Value>;

    fn next(&mut self) -> Option<Self::Item> {
        match (self.left.peek(), self.right.peek()) {
            (Some((left, _)), Some((right, _))) => match left.cmp(right) {
                Ordering::Less => {
                    let (key, value) = self.left.next().unwrap();
                    Some(Diff::Left(key, value))
                }
                Ordering::Equal => {
                    let (key, left) = self.left.next().unwrap();
                    let (_, right) = self.right.next().unwrap();
                    Some(Diff::Both(key, left.diff(right)))
                }
                Ordering::Greater => {
                    let (key, value) = self.right.next().unwrap();
                    Some(Diff::Right(key, value))
                }
            },
            (Some((_, _)), None) => {
                let (key, value) = self.left.next().unwrap();
                Some(Diff::Left(key, value))
            }
            (None, Some((_, _))) => {
                let (key, value) = self.right.next().unwrap();
                Some(Diff::Right(key, value))
            }
            (None, None) => None,
        }
    }
}

impl Diffable for Data {
    type Diff = DataDiff;

    fn diff(self, other: Self) -> Self::Diff {
        DataDiff {
            crates: DiffMap::new(self.crates, other.crates),
        }
    }
}

impl Diffable for Crate {
    type Diff = CrateDiff;

    fn diff(self, other: Self) -> Self::Diff {
        CrateDiff {
            releases: DiffMap::new(self.releases, other.releases),
        }
    }
}

impl Diffable for Release {
    type Diff = ReleaseDiff;

    fn diff(self, _other: Self) -> Self::Diff {
        ReleaseDiff {}
    }
}
