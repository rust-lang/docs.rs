use super::data::Crate;
use crate::db::types::version::Version;
use itertools::{
    EitherOrBoth::{Both, Left, Right},
    Itertools,
};
use std::fmt::Display;

#[derive(Debug, PartialEq)]
pub(super) enum Difference {
    CrateNotInIndex(String),
    CrateNotInDb(String, Vec<Version>),
    ReleaseNotInIndex(String, Version),
    ReleaseNotInDb(String, Version),
    ReleaseYank(String, Version, bool),
}

impl Display for Difference {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Difference::CrateNotInIndex(name) => {
                write!(f, "Crate in db not in index: {name}")?;
            }
            Difference::CrateNotInDb(name, _versions) => {
                write!(f, "Crate in index not in db: {name}")?;
            }
            Difference::ReleaseNotInIndex(name, version) => {
                write!(f, "Release in db not in index: {name} {version}")?;
            }
            Difference::ReleaseNotInDb(name, version) => {
                write!(f, "Release in index not in db: {name} {version}")?;
            }
            Difference::ReleaseYank(name, version, yanked) => {
                write!(
                    f,
                    "release yanked difference, index yanked:{yanked}, release: {name} {version}",
                )?;
            }
        }
        Ok(())
    }
}

pub(super) fn calculate_diff<'a, I>(db_data: I, index_data: I) -> Vec<Difference>
where
    I: Iterator<Item = &'a Crate>,
{
    let mut result = Vec::new();

    for crates_diff in db_data.merge_join_by(index_data, |db, index| db.name.cmp(&index.name)) {
        match crates_diff {
            Both(db_crate, index_crate) => {
                for release_diff in db_crate
                    .releases
                    .iter()
                    .merge_join_by(index_crate.releases.iter(), |db_release, index_release| {
                        db_release.version.cmp(&index_release.version)
                    })
                {
                    match release_diff {
                        Both(db_release, index_release) => {
                            let index_yanked =
                                index_release.yanked.expect("index always has yanked-state");
                            // if `db_release.yanked` is `None`, the record
                            // is coming from the build queue, not the `releases`
                            // table.
                            // In this case, we skip this check.
                            if let Some(db_yanked) = db_release.yanked
                                && db_yanked != index_yanked
                            {
                                result.push(Difference::ReleaseYank(
                                    db_crate.name.clone(),
                                    db_release.version.clone(),
                                    index_yanked,
                                ));
                            }
                        }
                        Left(db_release) => result.push(Difference::ReleaseNotInIndex(
                            db_crate.name.clone(),
                            db_release.version.clone(),
                        )),
                        Right(index_release) => result.push(Difference::ReleaseNotInDb(
                            index_crate.name.clone(),
                            index_release.version.clone(),
                        )),
                    }
                }
            }
            Left(db_crate) => result.push(Difference::CrateNotInIndex(db_crate.name.clone())),
            Right(index_crate) => result.push(Difference::CrateNotInDb(
                index_crate.name.clone(),
                index_crate
                    .releases
                    .iter()
                    .map(|r| r.version.clone())
                    .collect(),
            )),
        };
    }

    result
}

#[cfg(test)]
mod tests {
    use crate::test::{V2, V3};

    use super::super::data::Release;
    use super::*;
    use std::iter;

    #[test]
    fn test_empty() {
        assert!(calculate_diff(iter::empty(), iter::empty()).is_empty());
    }

    #[test]
    fn test_crate_not_in_index() {
        let db_releases = [Crate {
            name: "krate".into(),
            releases: vec![],
        }];

        assert_eq!(
            calculate_diff(db_releases.iter(), [].iter()),
            vec![Difference::CrateNotInIndex("krate".into())]
        );
    }

    #[test]
    fn test_crate_not_in_db() {
        let index_releases = [Crate {
            name: "krate".into(),
            releases: vec![
                Release {
                    version: V2,
                    yanked: Some(false),
                },
                Release {
                    version: V3,
                    yanked: Some(true),
                },
            ],
        }];

        assert_eq!(
            calculate_diff([].iter(), index_releases.iter()),
            vec![Difference::CrateNotInDb("krate".into(), vec![V2, V3])]
        );
    }

    #[test]
    fn test_yank_diff() {
        let db_releases = [Crate {
            name: "krate".into(),
            releases: vec![
                Release {
                    version: V2,
                    yanked: Some(true),
                },
                Release {
                    version: V3,
                    yanked: Some(true),
                },
            ],
        }];
        let index_releases = [Crate {
            name: "krate".into(),
            releases: vec![
                Release {
                    version: V2,
                    yanked: Some(false),
                },
                Release {
                    version: V3,
                    yanked: Some(true),
                },
            ],
        }];

        assert_eq!(
            calculate_diff(db_releases.iter(), index_releases.iter()),
            vec![Difference::ReleaseYank("krate".into(), V2, false,)]
        );
    }

    #[test]
    fn test_yank_diff_without_db_data() {
        let db_releases = [Crate {
            name: "krate".into(),
            releases: vec![Release {
                version: V2,
                yanked: None,
            }],
        }];
        let index_releases = [Crate {
            name: "krate".into(),
            releases: vec![Release {
                version: V2,
                yanked: Some(false),
            }],
        }];

        assert!(calculate_diff(db_releases.iter(), index_releases.iter()).is_empty());
    }
}
