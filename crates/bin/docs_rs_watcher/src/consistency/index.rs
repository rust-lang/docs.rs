use super::data::{Crate, Crates, Release, Releases};
use crate::Index;
use anyhow::Result;
use chrono::{DateTime, Utc};
use crates_index_diff::gix::ObjectId;
use docs_rs_types::{KrateName, Version};
use rayon::iter::ParallelIterator;
use std::collections::HashSet;

pub(super) async fn load(
    index: &mut Index<'_>,
    existing_crates: Option<HashSet<KrateName>>,
    reference: Option<ObjectId>,
) -> Result<Crates> {
    index
        .with_full_index_at(reference, move |full_index| {
            let mut result: Crates = full_index
                .crates_parallel()
                .filter_map(|krate| {
                    let krate = match krate {
                        Ok(krate) => krate,
                        Err(err) => return Some(Err(err)),
                    };

                    let name: KrateName = krate
                        .name()
                        .parse()
                        .expect("all crate names in the index are valid");

                    if let Some(existing_crates) = &existing_crates
                        && !existing_crates.contains(&name)
                    {
                        return None;
                    }

                    Some(Ok(Crate {
                        name,
                        releases: releases_from_index(&krate),
                    }))
                })
                .collect::<Result<_, _>>()?;

            result.sort_by(|lhs, rhs| lhs.name.cmp(&rhs.name));

            Ok(result)
        })
        .await
}

pub(super) async fn load_single(
    index: &mut Index<'_>,
    name: &KrateName,
    reference: Option<ObjectId>,
) -> Result<Option<Crate>> {
    let name = name.clone();
    index
        .with_full_index_at(reference, move |full_index| {
            Ok(full_index.crate_(name.as_str()).map(|krate| Crate {
                name,
                releases: releases_from_index(&krate),
            }))
        })
        .await
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
