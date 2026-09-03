use super::data::{Crate, Crates, Release, Releases};
use crate::Config;
use anyhow::Result;
use chrono::{DateTime, Utc};
use docs_rs_registry_api::RegistryApi;
use docs_rs_types::{KrateName, Version};
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
                    let mut releases = releases_from_index(&krate);

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

pub(super) async fn load_single(
    registry_api: &RegistryApi,
    name: &KrateName,
) -> Result<Option<Crate>> {
    let Some(krate) = registry_api.get_crate_from_index(name).await? else {
        return Ok(None);
    };

    let releases = releases_from_index(&krate);

    Ok(Some(Crate {
        name: name.clone(),
        releases,
    }))
}

fn releases_from_index(krate: &crates_index::Crate) -> Releases {
    let mut releases: Releases = krate
        .versions()
        .iter()
        .filter_map(|index_version| {
            index_version
                .version()
                .parse::<Version>()
                .ok()
                .map(|version| Release {
                    version,
                    yanked: Some(index_version.is_yanked()),
                    release_time: index_version
                        .pubtime()
                        .and_then(|time| time.parse::<DateTime<Utc>>().ok()),
                })
        })
        .collect();
    releases.sort_by(|lhs, rhs| lhs.version.cmp(&rhs.version));
    releases
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{DateTime, Utc};

    #[test]
    fn releases_include_index_pubtime() {
        let krate = crates_index::Crate::from_slice(
            br#"{"name":"krate","vers":"1.0.0","deps":[],"cksum":"0000000000000000000000000000000000000000000000000000000000000000","features":{},"yanked":false,"pubtime":"2024-01-02T03:04:05Z"}"#,
        )
        .unwrap();

        assert_eq!(
            releases_from_index(&krate)[0].release_time,
            Some("2024-01-02T03:04:05Z".parse::<DateTime<Utc>>().unwrap())
        );
    }
}
