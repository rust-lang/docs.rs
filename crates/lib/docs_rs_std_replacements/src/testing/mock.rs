use crate::{ReplacementDetails, ReplacementMap, StdReplacementsProvider};
use anyhow::Result;
use async_trait::async_trait;
use docs_rs_types::KrateName;
use std::sync::{Arc, Mutex};

/// In-memory replacement data for tests. Unknown crates return `None`.
#[derive(Debug, Default)]
pub struct MockStdReplacements {
    replacements: Mutex<ReplacementMap>,
}

impl MockStdReplacements {
    /// Create an empty client.
    pub fn new() -> Self {
        Self::default()
    }

    /// Add or replace a crate's entry, returning the previous details if present.
    pub fn insert<K>(&self, name: K, details: ReplacementDetails) -> Option<Arc<ReplacementDetails>>
    where
        K: TryInto<KrateName>,
        <K as TryInto<KrateName>>::Error: std::fmt::Debug,
    {
        let name = name.try_into().expect("invalid crate name");

        let mut replacements = self.replacements.lock().unwrap();
        replacements.insert(name, Arc::new(details))
    }

    /// Add an entry while constructing the client.
    #[cfg(test)]
    fn with_replacement<K>(self, name: K, details: ReplacementDetails) -> Self
    where
        K: TryInto<KrateName>,
        <K as TryInto<KrateName>>::Error: std::fmt::Debug,
    {
        self.insert(name, details);
        self
    }
}

#[async_trait]
impl StdReplacementsProvider for MockStdReplacements {
    async fn get(&self, name: &KrateName) -> Result<Option<Arc<ReplacementDetails>>> {
        let replacements = self.replacements.lock().unwrap();
        Ok(replacements.get(name).cloned())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::StdReplacements;
    use crate::testing::std_replacement;
    use docs_rs_types::testing::KRATE;

    #[tokio::test]
    async fn test_mock_can_be_injected_as_shared_client() -> anyhow::Result<()> {
        let details = std_replacement("replacement");
        let mock = MockStdReplacements::new().with_replacement(KRATE, std_replacement("old"));
        assert_eq!(
            mock.insert(KRATE, details.clone()).unwrap().description(),
            "old"
        );
        assert!(
            mock.insert(KrateName::from_static("another"), details.clone())
                .is_none()
        );

        let client: StdReplacements = Arc::new(mock);
        assert_eq!(*client.get(&KRATE).await?.unwrap(), details);
        assert_eq!(
            *client
                .get(&KrateName::from_static("another"))
                .await?
                .unwrap(),
            details
        );
        assert!(
            client
                .get(&KrateName::from_static("missing"))
                .await?
                .is_none()
        );
        Ok(())
    }
}
