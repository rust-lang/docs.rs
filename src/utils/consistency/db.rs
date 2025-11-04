use super::data::{Crate, Crates, Release, Releases};
use crate::{Config, db::types::version::Version};
use anyhow::Result;
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
         ORDER BY name, version"#,
        config.build_attempts as i32,
    )
    .fetch_all(conn)
    .await?;

    let mut crates = Crates::new();

    for (crate_name, release_rows) in &rows.iter().chunk_by(|row| row.name.clone()) {
        let releases: Releases = release_rows
            .map(|row| Release {
                version: row.version.clone(),
                yanked: row.yanked,
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
    use crate::test::{V1, V2, V3, async_wrapper};

    #[test]
    fn test_load() {
        async_wrapper(|env| async move {
            env.async_build_queue()
                .add_crate("queued", &V1, 0, None)
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

            let mut conn = env.async_db().async_conn().await;
            let result = load(&mut conn, env.config()).await?;

            assert_eq!(
                result,
                vec![
                    Crate {
                        name: "krate".into(),
                        releases: vec![
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
