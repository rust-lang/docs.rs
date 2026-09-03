use super::data::Crate;
use chrono::{DateTime, Utc};
use docs_rs_types::{KrateName, Version};
use itertools::{
    EitherOrBoth::{Both, Left, Right},
    Itertools,
};
use std::fmt::{self, Display};

#[derive(Debug, PartialEq)]
pub(super) enum Difference {
    CrateNotInIndex(KrateName),
    CrateNotInDb(KrateName, Vec<Version>),
    ReleaseNotInIndex(KrateName, Version),
    ReleaseNotInDb(KrateName, Version),
    ReleaseYank(KrateName, Version, bool),
    ReleaseTime(KrateName, Version, DateTime<Utc>),
}

impl Display for Difference {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
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
            Difference::ReleaseTime(name, version, release_time) => {
                write!(
                    f,
                    "release time difference, index release time: {release_time}, release: {name} {version}",
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

                            // NOTE: `yanked` and `release_time` come both from the
                            // crates.io sparse index, or historically from the crates.io API.
                            // We might have releases where both fields are empty because of an
                            // error, or because the release build is still in progress.
                            // So there might be cases where `release_time` was empty because
                            // it was empty on the index (unlikely), or we might have cases
                            // where the releases is still in progress.
                            // Since `yanked` is mandatory on the sparse index, we can use that
                            // as an indicator that `release_time` can be overwritten.
                            if db_release.yanked.is_some()
                                && let Some(index_release_time) = index_release.release_time
                                && db_release.release_time != Some(index_release_time)
                            {
                                result.push(Difference::ReleaseTime(
                                    db_crate.name.clone(),
                                    db_release.version.clone(),
                                    index_release_time,
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
    use super::super::data::Release;
    use super::*;
    use chrono::DateTime;
    use docs_rs_types::testing::{KRATE, V2, V3};
    use std::iter;

    #[test]
    fn test_empty() {
        assert!(calculate_diff(iter::empty(), iter::empty()).is_empty());
    }

    #[test]
    fn test_crate_not_in_index() {
        let db_releases = [Crate {
            name: KRATE,
            releases: vec![],
        }];

        assert_eq!(
            calculate_diff(db_releases.iter(), [].iter()),
            vec![Difference::CrateNotInIndex(KRATE)]
        );
    }

    #[test]
    fn test_crate_not_in_db() {
        let index_releases = [Crate {
            name: KRATE,
            releases: vec![
                Release {
                    version: V2,
                    yanked: Some(false),
                    release_time: None,
                },
                Release {
                    version: V3,
                    yanked: Some(true),
                    release_time: None,
                },
            ],
        }];

        assert_eq!(
            calculate_diff([].iter(), index_releases.iter()),
            vec![Difference::CrateNotInDb(KRATE, vec![V2, V3])]
        );
    }

    #[test]
    fn test_yank_diff() {
        let db_releases = [Crate {
            name: KRATE,
            releases: vec![
                Release {
                    version: V2,
                    yanked: Some(true),
                    release_time: None,
                },
                Release {
                    version: V3,
                    yanked: Some(true),
                    release_time: None,
                },
            ],
        }];
        let index_releases = [Crate {
            name: KRATE,
            releases: vec![
                Release {
                    version: V2,
                    yanked: Some(false),
                    release_time: None,
                },
                Release {
                    version: V3,
                    yanked: Some(true),
                    release_time: None,
                },
            ],
        }];

        assert_eq!(
            calculate_diff(db_releases.iter(), index_releases.iter()),
            vec![Difference::ReleaseYank(KRATE, V2, false,)]
        );
    }

    #[test]
    fn test_yank_diff_without_db_data() {
        let db_releases = [Crate {
            name: KRATE,
            releases: vec![Release {
                version: V2,
                yanked: None,
                release_time: None,
            }],
        }];
        let index_releases = [Crate {
            name: KRATE,
            releases: vec![Release {
                version: V2,
                yanked: Some(false),
                release_time: Some("2024-01-01T00:00:00Z".parse::<DateTime<Utc>>().unwrap()),
            }],
        }];

        assert!(calculate_diff(db_releases.iter(), index_releases.iter()).is_empty());
    }

    #[test]
    fn test_release_time_diff() {
        let db_releases = [Crate {
            name: KRATE,
            releases: vec![Release {
                version: V2,
                yanked: Some(false),
                release_time: Some("2024-01-01T00:00:00Z".parse::<DateTime<Utc>>().unwrap()),
            }],
        }];
        let expected = "2024-01-02T00:00:00Z".parse::<DateTime<Utc>>().unwrap();
        let index_releases = [Crate {
            name: KRATE,
            releases: vec![Release {
                version: V2,
                yanked: Some(false),
                release_time: Some(expected),
            }],
        }];

        assert_eq!(
            calculate_diff(db_releases.iter(), index_releases.iter()),
            vec![Difference::ReleaseTime(KRATE, V2, expected)]
        );
    }

    #[test]
    fn test_missing_index_release_time_does_not_clear_database_value() {
        let db_releases = [Crate {
            name: KRATE,
            releases: vec![Release {
                version: V2,
                yanked: Some(false),
                release_time: Some("2024-01-01T00:00:00Z".parse::<DateTime<Utc>>().unwrap()),
            }],
        }];
        let index_releases = [Crate {
            name: KRATE,
            releases: vec![Release {
                version: V2,
                yanked: Some(false),
                release_time: None,
            }],
        }];

        assert!(calculate_diff(db_releases.iter(), index_releases.iter()).is_empty());
    }
}
