use anyhow::{Context as _, Result};
use docs_rs_build_queue::AsyncBuildQueue;
use docs_rs_database::service_config::{Abnormality, ConfigName, get_config};

pub(crate) type ActiveAbnormalities = Vec<Abnormality>;

pub(crate) async fn load_abnormalities(
    conn: &mut sqlx::PgConnection,
    build_queue: &AsyncBuildQueue,
) -> Result<ActiveAbnormalities> {
    let mut active_abnormalities = ActiveAbnormalities::new();

    if let Some(abnormality) = get_config::<Abnormality>(conn, ConfigName::Abnormality)
        .await
        .context("failed to load manual abnormality from config")?
    {
        active_abnormalities.push(abnormality);
    }

    active_abnormalities.extend(
        build_queue
            .gather_alerts()
            .await
            .context("failed to load build queue abnormalities")?,
    );

    Ok(active_abnormalities)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::TestEnvironment;
    use anyhow::Result;
    use docs_rs_config::AppConfig as _;
    use docs_rs_database::service_config::set_config;
    use docs_rs_types::{KrateName, Version};
    use docs_rs_uri::EscapedURI;

    #[tokio::test(flavor = "multi_thread")]
    async fn load_abnormalities_returns_manual_abnormality() -> Result<()> {
        let env = TestEnvironment::new().await?;

        let mut conn = env.async_conn().await?;
        let manual_abnormality = Abnormality {
            url: "https://example.com/maintenance"
                .parse::<EscapedURI>()
                .unwrap(),
            text: "Scheduled maintenance".into(),
            explanation: Some("Planned maintenance is in progress.".into()),
        };
        set_config(
            &mut conn,
            ConfigName::Abnormality,
            manual_abnormality.clone(),
        )
        .await?;

        let abnormalities = load_abnormalities(&mut conn, env.build_queue()?).await?;

        assert_eq!(abnormalities, vec![manual_abnormality]);
        Ok(())
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn load_abnormalities_returns_queue_abnormality() -> Result<()> {
        let mut queue_config = docs_rs_build_queue::Config::test_config()?;
        queue_config.length_warning_threshold = 1;
        let env = crate::testing::TestEnvironment::builder()
            .build_queue_config(queue_config)
            .build()
            .await?;

        let queue = env.build_queue()?.clone();
        for idx in 0..2 {
            let name = format!("queued-crate-{idx}").parse::<KrateName>()?;
            queue
                .add_crate(&name, &Version::parse("1.0.0")?, 0, None)
                .await?;
        }

        let mut conn = env.async_conn().await?;
        let abnormalities = load_abnormalities(&mut conn, &queue).await?;

        assert_eq!(
            abnormalities,
            vec![Abnormality {
                url: EscapedURI::from_path("/releases/queue"),
                text: "long build queue".into(),
                explanation: Some(
                    "The build queue currently contains more than 1 crates, so it might take a while before new published crates get documented.".into()
                ),
            }]
        );

        Ok(())
    }
}
