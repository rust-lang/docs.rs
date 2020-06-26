use super::data::{Crate, CrateId, Data, Release, Version};
use crate::{config::Config, index::Index};

pub(crate) fn load(config: &Config) -> Result<Data, failure::Error> {
    let index = Index::new(&config.registry_index_path)?;

    let mut data = Data::default();

    index.crates()?.walk(|krate| {
        data.crates.insert(
            CrateId(krate.name().into()),
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
