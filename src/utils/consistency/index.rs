use super::data::{Crate, Crates, Release, Releases};
use crate::Index;
use anyhow::Result;
use rayon::iter::ParallelIterator;

pub(super) fn load(index: &Index) -> Result<Crates> {
    let mut result: Crates = index
        .crates()?
        .crates_parallel()
        .map(|krate| {
            krate.map(|krate| {
                let mut releases: Releases = krate
                    .versions()
                    .iter()
                    .map(|version| Release {
                        version: version.version().into(),
                        yanked: Some(version.is_yanked()),
                    })
                    .collect();
                releases.sort_by(|lhs, rhs| lhs.version.cmp(&rhs.version));

                Crate {
                    name: krate.name().into(),
                    releases,
                }
            })
        })
        .collect::<Result<_, _>>()?;

    result.sort_by(|lhs, rhs| lhs.name.cmp(&rhs.name));

    Ok(result)
}
