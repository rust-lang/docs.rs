use super::data::{Crate, Crates, Release, Releases};
use anyhow::Result;
use docs_rs_types::{KrateName, Version};
use itertools::Itertools;

pub(super) async fn load(conn: &mut sqlx::PgConnection) -> Result<Crates> {
    let rows = sqlx::query!(
        r#"SELECT
            name as "name!: KrateName",
            version as "version!: Version",
            yanked,
            release_time
         FROM (
             SELECT
                 crates.name,
                 releases.version,
                 releases.yanked,
                 releases.release_time
             FROM crates
             INNER JOIN releases ON releases.crate_id = crates.id
             UNION ALL
             -- crates & releases that are already queued
             -- don't have to be requeued.
             SELECT
                 queue.name,
                 queue.version,
                 NULL as yanked,
                 NULL as release_time
             FROM queue
             LEFT OUTER JOIN crates ON crates.name = queue.name
             LEFT OUTER JOIN releases ON (
                 releases.crate_id = crates.id AND
                 releases.version = queue.version
             )
             WHERE (
                 crates.id IS NULL OR
                 releases.id IS NULL
             )
         ) AS inp
         ORDER BY name"#,
    )
    .fetch_all(conn)
    .await?;

    let mut crates = Crates::new();

    for (crate_name, release_rows) in &rows.iter().chunk_by(|row| row.name.clone()) {
        let mut releases: Releases = release_rows
            .map(|row| Release {
                version: row.version.clone(),
                yanked: row.yanked,
                release_time: row.release_time,
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

pub(super) async fn load_single(
    conn: &mut sqlx::PgConnection,
    name: &KrateName,
) -> Result<Option<Crate>> {
    let rows = sqlx::query!(
        r#"SELECT version as "version!: Version", yanked, release_time
           FROM (
               SELECT releases.version, releases.yanked, releases.release_time
               FROM crates
               INNER JOIN releases ON releases.crate_id = crates.id
               WHERE crates.name = $1
               UNION ALL
               SELECT queue.version, NULL as yanked, NULL as release_time
               FROM queue
               LEFT OUTER JOIN crates ON crates.name = queue.name
               LEFT OUTER JOIN releases ON (
                   releases.crate_id = crates.id AND
                   releases.version = queue.version
               )
               WHERE queue.name = $1
                 AND (crates.id IS NULL OR releases.id IS NULL)
           ) AS inp"#,
        name as _,
    )
    .fetch_all(conn)
    .await?;

    if rows.is_empty() {
        return Ok(None);
    }

    let mut releases: Releases = rows
        .into_iter()
        .map(|row| Release {
            version: row.version,
            yanked: row.yanked,
            release_time: row.release_time,
        })
        .collect();
    releases.sort_by(|lhs, rhs| lhs.version.cmp(&rhs.version));

    Ok(Some(Crate {
        name: name.clone(),
        releases,
    }))
}

#[cfg(test)]
mod tests {
    use crate::testing::TestEnvironment;

    use super::*;
    use chrono::{DateTime, Utc};
    use docs_rs_types::{
        KrateName,
        testing::{KRATE, V1, V2, V3},
    };
    use pretty_assertions::assert_eq;

    const QUEUED: KrateName = KrateName::from_static("queued");

    #[tokio::test(flavor = "multi_thread")]
    async fn test_load() -> Result<()> {
        let env = TestEnvironment::new().await?;
        let release_time = "2024-01-01T00:00:00Z".parse::<DateTime<Utc>>()?;

        env.build_queue()?.add_crate(&QUEUED, &V1, 0).await?;
        env.fake_release()
            .await
            .name("krate")
            .version(V2)
            .release_time(release_time)
            .create()
            .await?;
        env.fake_release()
            .await
            .name("krate")
            .version(V3)
            .yanked(true)
            .release_time(release_time)
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
            .release_time(release_time)
            .create()
            .await?;
        env.fake_release()
            .await
            .name("krate")
            .version(V0_10_3)
            .yanked(false)
            .release_time(release_time)
            .create()
            .await?;

        let mut conn = env.async_conn().await?;
        let result = load(&mut conn).await?;

        assert_eq!(
            result,
            vec![
                Crate {
                    name: KRATE,
                    releases: vec![
                        Release {
                            version: V0_9_3,
                            yanked: Some(false),
                            release_time: Some(release_time),
                        },
                        Release {
                            version: V0_10_3,
                            yanked: Some(false),
                            release_time: Some(release_time),
                        },
                        Release {
                            version: V2,
                            yanked: Some(false),
                            release_time: Some(release_time),
                        },
                        Release {
                            version: V3,
                            yanked: Some(true),
                            release_time: Some(release_time),
                        }
                    ]
                },
                Crate {
                    name: QUEUED,
                    releases: vec![Release {
                        version: V1,
                        yanked: None,
                        release_time: None,
                    }]
                },
            ]
        );
        Ok(())
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_load_single() -> Result<()> {
        let env = TestEnvironment::new().await?;
        let release_time = "2024-01-01T00:00:00Z".parse::<DateTime<Utc>>()?;

        env.fake_release()
            .await
            .name(KRATE)
            .version(V1)
            .release_time(release_time)
            .create()
            .await?;
        env.fake_release()
            .await
            .name("other")
            .version(V1)
            .create()
            .await?;
        env.build_queue()?.add_crate(&KRATE, &V2, 0).await?;

        let mut conn = env.async_conn().await?;
        assert_eq!(
            load_single(&mut conn, &KRATE).await?.unwrap(),
            Crate {
                name: KRATE,
                releases: vec![
                    Release {
                        version: V1,
                        yanked: Some(false),
                        release_time: Some(release_time),
                    },
                    Release {
                        version: V2,
                        yanked: None,
                        release_time: None,
                    },
                ],
            }
        );

        Ok(())
    }
}
