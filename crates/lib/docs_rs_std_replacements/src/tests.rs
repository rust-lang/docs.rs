use super::*;
use crate::testing::TestStdReplacements;
use docs_rs_types::testing::KRATE;
use reqwest::StatusCode;
use test_case::test_case;

fn std_replacement(description: &str) -> ReplacementDetails {
    ReplacementDetails {
        description: description.to_string(),
        url: "https://example.com/replacement".parse().unwrap(),
    }
}

fn std_replacements(
    replacements: impl IntoIterator<Item = (KrateName, ReplacementDetails)>,
) -> ReplacementMap {
    ReplacementMap::from_iter(
        replacements
            .into_iter()
            .map(|(krate, replacement)| (krate, Arc::new(replacement))),
    )
}

#[tokio::test]
async fn test_get_std_replacement_caches_entire_response() -> anyhow::Result<()> {
    let env = TestStdReplacements::new().await?;
    let details = std_replacement("replacement");
    env.mock_std_replacements(std_replacements([(KRATE, details.clone())]))
        .await;

    // A miss still fetches and caches the complete response.
    assert!(
        env.api()
            .get(&KrateName::from_static("missing"))
            .await?
            .is_none()
    );
    let first = env.api().get(&KRATE).await?.unwrap();
    assert_eq!(first, details.into());
    let second = env.api().get(&KRATE).await?.unwrap();
    assert!(Arc::ptr_eq(&first, &second));
    env.assert_mocks().await;
    Ok(())
}

#[tokio::test]
async fn test_get_std_replacement_caches_empty_response() -> anyhow::Result<()> {
    let env = TestStdReplacements::new().await?;
    env.mock_std_replacements(ReplacementMap::new()).await;

    for _ in 0..2 {
        assert!(env.api().get(&KRATE).await?.is_none());
    }
    env.assert_mocks().await;
    Ok(())
}

#[tokio::test]
async fn test_get_std_replacement_refreshes_expired_cache() -> anyhow::Result<()> {
    let env = TestStdReplacements::new().await?;

    let removed = KrateName::from_static("removed");
    env.mock_std_replacements(std_replacements([
        (KRATE, std_replacement("old")),
        (removed.clone(), std_replacement("removed")),
    ]))
    .await;

    let old = env.api().get(&KRATE).await?.unwrap();

    env.mock_std_replacements(std_replacements([(KRATE, std_replacement("new"))]))
        .await;
    env.api().cache.lock().await.as_mut().unwrap().fetched_at = Instant::now() - CACHE_TTL;

    let new = env.api().get(&KRATE).await?.unwrap();
    assert_eq!(new.description, "new");
    assert!(!Arc::ptr_eq(&old, &new));
    assert_eq!(old.description, "old");
    assert!(env.api().get(&removed).await?.is_none());
    let cached = env.api().get(&KRATE).await?.unwrap();
    assert!(Arc::ptr_eq(&new, &cached));
    env.assert_mocks().await;
    Ok(())
}

#[tokio::test]
async fn test_get_std_replacement_concurrent_fetch() -> anyhow::Result<()> {
    let env = TestStdReplacements::new().await?;
    env.mock_std_replacements(std_replacements([(KRATE, std_replacement("replacement"))]))
        .await;

    let name = KRATE;
    let (first, second) = tokio::try_join!(env.api().get(&name), env.api().get(&name),)?;
    assert!(Arc::ptr_eq(&first.unwrap(), &second.unwrap()));
    env.assert_mocks().await;
    Ok(())
}

#[test_case(StatusCode::NOT_FOUND, "not found"; "http error")]
#[test_case(StatusCode::INTERNAL_SERVER_ERROR, "server error"; "server error")]
#[test_case(StatusCode::OK, "invalid json"; "malformed json")]
#[test_case(StatusCode::OK, r#"{"krate":{"description":"missing url"}}"#; "invalid details")]
#[tokio::test]
async fn test_get_std_replacement_failed_fetch(
    status: StatusCode,
    body: &str,
) -> anyhow::Result<()> {
    let env = TestStdReplacements::new().await?;
    env.create_std_replacements_mock(|mock| {
        mock.with_status(status.as_u16().into()).with_body(body)
    })
    .await;

    let err = env.api().get(&KRATE).await.unwrap_err();
    if status.is_success() {
        assert!(err.downcast_ref::<reqwest::Error>().unwrap().is_decode());
    } else {
        assert_eq!(
            err.downcast_ref::<reqwest::Error>().unwrap().status(),
            Some(status)
        );
    }
    assert!(env.api().cache.lock().await.is_none());

    env.mock_std_replacements(std_replacements([(KRATE, std_replacement("recovered"))]))
        .await;
    let recovered = env.api().get(&KRATE).await?.unwrap();
    assert_eq!(recovered.description, "recovered");
    env.assert_mocks().await;
    Ok(())
}

#[tokio::test]
async fn test_get_std_replacement_failed_refresh_preserves_cache() -> anyhow::Result<()> {
    let env = TestStdReplacements::new().await?;
    env.mock_std_replacements(std_replacements([(KRATE, std_replacement("old"))]))
        .await;
    let old = env.api().get(&KRATE).await?.unwrap();
    let expired_at = Instant::now() - CACHE_TTL;
    env.api().cache.lock().await.as_mut().unwrap().fetched_at = expired_at;

    env.create_std_replacements_mock(|mock| {
        mock.with_status(StatusCode::INTERNAL_SERVER_ERROR.as_u16().into())
    })
    .await;
    assert!(env.api().get(&KRATE).await.is_err());
    {
        let cache = env.api().cache.lock().await;
        let cache = cache.as_ref().unwrap();
        assert_eq!(cache.fetched_at, expired_at);
        assert!(Arc::ptr_eq(&cache.data[&KRATE], &old));
    }

    env.mock_std_replacements(std_replacements([(KRATE, std_replacement("recovered"))]))
        .await;
    let recovered = env.api().get(&KRATE).await?.unwrap();
    assert_eq!(recovered.description, "recovered");
    env.assert_mocks().await;
    Ok(())
}
