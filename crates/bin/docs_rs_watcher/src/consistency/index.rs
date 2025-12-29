use super::data::{Crate, Crates, Release, Releases};
use crate::Config;
use anyhow::Result;
use docs_rs_types::Version;
use docs_rs_utils::run_blocking;
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
                    let mut releases: Releases =
                        krate
                            .versions()
                            .iter()
                            .filter_map(|version| {
                                version.version().parse::<Version>().ok().map(|semversion| {
                                    Release {
                                        version: semversion,
                                        yanked: Some(version.is_yanked()),
                                    }
                                })
                            })
                            .collect();

                    releases.sort_by(|lhs, rhs| lhs.version.cmp(&rhs.version));

                    Crate {
                        name: krate
                            .name()
                            .parse()
                            .expect("all crate names in the index vare valid"),
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
