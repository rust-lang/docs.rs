use super::data::{Crate, CrateName, Data, Release, Version};
use crate::Index;

pub(crate) fn load(index: &Index) -> Result<Data, failure::Error> {
    let mut data = Data::default();

    index.crates()?.walk(|krate| {
        data.crates.insert(
            CrateName(krate.name().into()),
            Crate {
                releases: krate
                    .versions()
                    .iter()
                    .map(|version| (Version(version.version().into()), Release::default()))
                    .collect(),
            },
        );
    })?;

    Ok(data)
}
