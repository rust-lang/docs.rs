use super::data::{Crate, Crates, Release, Releases};
use crate::Config;
use anyhow::Result;
use docs_rs_types::{KrateName, Version};
use docs_rs_uri::EscapedURI;
use docs_rs_utils::{APP_USER_AGENT, run_blocking};
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

pub(super) async fn load_single(name: &KrateName) -> Result<Crates> {
    let url = sparse_index_url(name);
    let response = reqwest::Client::builder()
        .user_agent(APP_USER_AGENT)
        .build()?
        .get(url.to_string())
        .send()
        .await?;

    if response.status() == reqwest::StatusCode::NOT_FOUND {
        return Ok(Vec::new());
    }

    let bytes = response.error_for_status()?.bytes().await?;
    let krate = crates_index::Crate::from_slice(&bytes)?;
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
                })
        })
        .collect();
    releases.sort_by(|lhs, rhs| lhs.version.cmp(&rhs.version));

    Ok(vec![Crate {
        name: name.clone(),
        releases,
    }])
}

fn sparse_index_url(name: &KrateName) -> EscapedURI {
    let name = name.as_str().to_ascii_lowercase();
    let path = match name.len() {
        1 => format!("1/{name}"),
        2 => format!("2/{name}"),
        3 => format!("3/{}/{name}", &name[..1]),
        _ => format!("{}/{}/{name}", &name[..2], &name[2..4]),
    };
    format!("https://index.crates.io/{path}")
        .parse()
        .expect("the sparse index URL is valid")
}

#[cfg(test)]
mod tests {
    use super::*;
    use test_case::test_case;

    #[test_case("a", "https://index.crates.io/1/a")]
    #[test_case("ab", "https://index.crates.io/2/ab")]
    #[test_case("abc", "https://index.crates.io/3/a/abc")]
    #[test_case("Serde", "https://index.crates.io/se/rd/serde")]
    fn sparse_index_urls(name: &str, expected: &str) {
        assert_eq!(sparse_index_url(&name.parse().unwrap()), expected);
    }
}
