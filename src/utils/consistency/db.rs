use super::data::{Crate, Crates, Release, Releases};
use crate::Config;
use anyhow::Result;
use docs_rs_types::Version;
use itertools::Itertools;

pub(super) async fn load(conn: &mut sqlx::PgConnection, config: &Config) -> Result<Crates> {
    let rows = sqlx::query!(
        r#"SELECT
            name as "name!",
            version as "version!: Version",
            yanked
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
             SELECT
                 queue.name,
                 queue.version,
                 NULL as yanked
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
         ORDER BY name"#,
        config.build_queue.build_attempts as i32,
    )
    .fetch_all(conn)
    .await?;

    let mut crates = Crates::new();

    for (crate_name, release_rows) in &rows.iter().chunk_by(|row| row.name.clone()) {
        let mut releases: Releases = release_rows
            .map(|row| Release {
                version: row.version.clone(),
                yanked: row.yanked,
            })
            .collect();

        releases.sort_by(|lhs, rhs| lhs.version.cmp(&rhs.version));

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
    use crate::test::{V1, V2, V3, async_wrapper};
    use docs_rs_types::KrateName;
    use pretty_assertions::assert_eq;

    const QUEUED: KrateName = KrateName::from_static("queued");

    #[test]
    fn test_load() {
        async_wrapper(|env| async move {
            env.async_build_queue()
                .add_crate(&QUEUED, &V1, 0, None)
                .await?;
            env.fake_release()
                .await
                .name("krate")
                .version(V2)
                .create()
                .await?;
            env.fake_release()
                .await
                .name("krate")
                .version(V3)
                .yanked(true)
                .create()
                .await?;

            // these two releases are there to ensure we sort correctly.
            // In the past, we sorted the version (from the crates index & our database)
            // as string, which lead to "0.10.3" coming before "0.9.3".
            // When both sides are sorted the same way, this is fine and doesn't break the
            // consistency check.
            // But after migrating everything to using `semver::Version`, the sorting changed
            // on the index-side, while we still sorted by string on the database side.
            //
            // Since I still run the consistency check manually, every now and then, this wasn't
            // an issue, because I saw the odd huge difference.
            //
            // The solution is to sort both sides semver correctly.
            const V0_9_3: Version = Version::new(0, 9, 3);
            const V0_10_3: Version = Version::new(0, 10, 3);
            env.fake_release()
                .await
                .name("krate")
                .version(V0_9_3)
                .yanked(false)
                .create()
                .await?;
            env.fake_release()
                .await
                .name("krate")
                .version(V0_10_3)
                .yanked(false)
                .create()
                .await?;

            let mut conn = env.async_db().async_conn().await;
            let result = load(&mut conn, env.config()).await?;

            assert_eq!(
                result,
                vec![
                    Crate {
                        name: "krate".into(),
                        releases: vec![
                            Release {
                                version: V0_9_3,
                                yanked: Some(false),
                            },
                            Release {
                                version: V0_10_3,
                                yanked: Some(false),
                            },
                            Release {
                                version: V2,
                                yanked: Some(false),
                            },
                            Release {
                                version: V3,
                                yanked: Some(true),
                            }
                        ]
                    },
                    Crate {
                        name: "queued".into(),
                        releases: vec![Release {
                            version: V1,
                            yanked: None,
                        }]
                    },
                ]
            );
            Ok(())
        })
    }
}
