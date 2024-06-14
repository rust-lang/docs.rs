use super::data::{Crate, Crates, Release, Releases};
use crate::Config;
use anyhow::Result;
use itertools::Itertools;
use postgres::fallible_iterator::FallibleIterator;
use std::iter;

pub(super) fn load(conn: &mut postgres::Client, config: &Config) -> Result<Crates> {
    let rows = conn.query_raw(
        "SELECT name, version, yanked
         FROM (
             SELECT
                 crates.name,
                 releases.version,
                 releases.yanked
             FROM crates
             INNER JOIN releases ON releases.crate_id = crates.id
             UNION ALL
             -- crates & releases that are already queued
             -- don't have to be requeued.
             SELECT queue.name, queue.version, NULL as yanked
             FROM queue
             LEFT OUTER JOIN crates ON crates.name = queue.name
             LEFT OUTER JOIN releases ON (
                 releases.crate_id = crates.id AND
                 releases.version = queue.version
             )
             WHERE queue.attempt < $1 AND (
                 crates.id IS NULL OR
                 releases.id IS NULL
             )
         ) AS inp
         ORDER BY name, version",
        iter::once(config.build_attempts as i32),
    )?;

    let mut crates = Crates::new();

    for (crate_name, release_rows) in &rows
        // `rows` is a `FallibleIterator` which needs to be converted before
        // we can use Itertools.group_by on it.
        .iterator()
        .map(|row| row.expect("error fetching rows"))
        .chunk_by(|row| row.get("name"))
    {
        let releases: Releases = release_rows
            .map(|row| Release {
                version: row.get("version"),
                yanked: row.get("yanked"),
            })
            .collect();

        crates.push(Crate {
            name: crate_name,
            releases,
        });
    }

    Ok(crates)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test::wrapper;

    #[test]
    fn test_load() {
        wrapper(|env| {
            env.build_queue().add_crate("queued", "0.0.1", 0, None)?;
            env.fake_release().name("krate").version("0.0.2").create()?;
            env.fake_release()
                .name("krate")
                .version("0.0.3")
                .yanked(true)
                .create()?;

            assert_eq!(
                load(&mut env.db().conn(), &env.config())?,
                vec![
                    Crate {
                        name: "krate".into(),
                        releases: vec![
                            Release {
                                version: "0.0.2".into(),
                                yanked: Some(false),
                            },
                            Release {
                                version: "0.0.3".into(),
                                yanked: Some(true),
                            }
                        ]
                    },
                    Crate {
                        name: "queued".into(),
                        releases: vec![Release {
                            version: "0.0.1".into(),
                            yanked: None,
                        }]
                    },
                ]
            );
            Ok(())
        })
    }
}
