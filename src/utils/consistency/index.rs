use super::data::{Crate, CrateName, Data, Release, Version};
use crate::Index;
use rayon::iter::ParallelIterator;

pub(crate) fn load(index: &Index) -> Result<Data, anyhow::Error> {
    let crates = index
        .crates()?
        .crates_parallel()
        .map(|krate| {
            krate.map(|krate| {
                let releases = krate
                    .versions()
                    .iter()
                    .map(|version| (Version(version.version().into()), Release::default()))
                    .collect();
                (CrateName(krate.name().into()), Crate { releases })
            })
        })
        .collect::<Result<_, _>>()?;

    Ok(Data { crates })
}
