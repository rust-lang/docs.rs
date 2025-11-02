use super::data::{Crate, Crates, Release, Releases};
use crate::{Config, utils::run_blocking};
use anyhow::Result;
use rayon::iter::ParallelIterator;
use tracing::debug;

pub(super) async fn load(config: &Config) -> Result<Crates> {
    let registry_index_path = config.registry_index_path.clone();
    let registry_url = config
        .registry_url
        .as_deref()
        .unwrap_or("https://github.com/rust-lang/crates.io-index")
        .to_owned();

    run_blocking("load-crates-index", move || {
        debug!("Opening with `crates_index`");
        let mut index = crates_index::GitIndex::with_path(
            &registry_index_path,
            // crates_index requires the repo url to match the existing origin or it tries to reinitialize the repo
            &registry_url,
        )?;

        index.update()?;

        let mut result: Crates = index
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
    })
    .await
}
