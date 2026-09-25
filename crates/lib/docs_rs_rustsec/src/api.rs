use crate::Config;
use anyhow::{Context as _, Result};
use arc_swap::ArcSwapOption;
use docs_rs_types::KrateName;
use docs_rs_utils::spawn_blocking;
use rustsec::advisory::Informational;
use std::{path::PathBuf, sync::Arc, time::Duration};
use tokio::task::JoinHandle;
use tracing::{error, info, instrument, warn};

fn crate_query(name: &KrateName) -> rustsec::database::Query {
    rustsec::database::Query::new()
        .collection(rustsec::Collection::Crates)
        .package_name(
            name.as_str()
                .parse()
                .expect("will never fail because we use KrateName"),
        )
}

/// and opened & loaded rustsec advisory db.
///
/// NOTE: While the real advisory db is a git repo and stored locally on
/// disk, `rustsec::Database` loads everything into memory on `::open`.
#[derive(Debug)]
pub struct RustsecDatabase(pub(crate) Arc<rustsec::Database>);

impl RustsecDatabase {
    /// Fetch the first unmaintained advisory that has not been withdrawn and
    /// does not offer a patched version, matching crates.io.
    ///
    /// Returns `None` when the database has not loaded or no advisory matches.
    #[instrument(skip(self), fields(krate = %name))]
    pub fn find_unmaintained(&self, name: &KrateName) -> Option<&rustsec::Advisory> {
        let query = crate_query(name).withdrawn(false).informational(true);

        self.0
            .query(&query)
            .into_iter()
            .filter(|advisory| {
                advisory.metadata.informational == Some(Informational::Unmaintained)
                    && advisory.versions.patched().is_empty()
            })
            // `min_by_key` instead of `find` so the order is deterministic
            .min_by_key(|advisory| advisory.id())
    }
}

type DatabasePtr = Arc<ArcSwapOption<rustsec::Database>>;

/// global rustsec client.
///
/// Manages our local copy of the advisory-db git repo,
/// and it's regular refreshes.
#[derive(Debug)]
pub struct RustsecClient {
    database: DatabasePtr,
    background_task: Option<JoinHandle<()>>,
}

impl RustsecClient {
    pub async fn from_config(config: &Config) -> Result<Self> {
        let database = Arc::new(ArcSwapOption::empty());

        // we first try to open an existing local database at the configured location.
        // (either custom folder, or the rustsec standard folder).
        // `rustsec::Database` loads _everything_ into memory on open. At the time of writing
        // it takes ~20ms to load ~1250 advisories. 20ms is ok to add to our server startup time.
        //
        // The background thread will replace that DB later with an updated one.
        match spawn_blocking({
            let advisory_db = config.advisory_db.clone();
            move || Ok(rustsec::Database::open(&advisory_db)?)
        })
        .await
        {
            Ok(loaded) => {
                info!(
                    path = %config.advisory_db.display(),
                    entries = loaded.iter().count(),
                    "loaded existing RustSec advisory database"
                );
                database.store(Some(Arc::new(loaded)));
            }
            Err(error) => {
                warn!(
                    ?error,
                    path = %config.advisory_db.display(),
                    "failed to open existing RustSec advisory database; fetching it in the background"
                );
            }
        }

        let background_task = tokio::spawn({
            let database = database.clone();
            let path = config.advisory_db.clone();
            let refresh_frequency: Duration = config.refresh_frequency.into();

            async move {
                loop {
                    if let Err(err) = Self::refresh(path.clone(), &database).await {
                        error!(?err, "failed to load RustSec database");
                    }

                    tokio::time::sleep(refresh_frequency).await;
                }
            }
        });

        Ok(Self {
            database,
            background_task: Some(background_task),
        })
    }

    /// Create a client backed by an already loaded database.
    ///
    /// This is useful for consumers' tests, which should not need to configure
    /// or fetch a RustSec repository.
    #[cfg(any(test, feature = "testing"))]
    pub fn from_database(database: RustsecDatabase) -> Self {
        let pointer = Arc::new(ArcSwapOption::empty());
        pointer.store(Some(database.0));

        Self {
            database: pointer,
            background_task: None,
        }
    }

    #[instrument(skip_all, fields(path = %path.display()))]
    async fn refresh(path: PathBuf, database: &DatabasePtr) -> Result<()> {
        info!("refreshing rustsec advisory database");
        let loaded = spawn_blocking(move || {
            let repo = rustsec::Repository::fetch(
                rustsec::repository::git::DEFAULT_URL,
                path.clone(),
                true,
                Duration::from_mins(5),
            )
            .with_context(|| format!("failed to fetch RustSec database at {}", path.display()))?;
            rustsec::Database::load_from_repo(&repo)
                .context("failed to load fetched RustSec database")
        })
        .await?;
        info!(entries = loaded.iter().count(), "...done");
        database.store(Some(Arc::new(loaded)));
        Ok(())
    }

    /// gives you the database for queries, if it's already loaded.
    pub fn database(&self) -> Option<RustsecDatabase> {
        self.database.load().clone().map(RustsecDatabase)
    }
}

impl Drop for RustsecClient {
    fn drop(&mut self) {
        if let Some(background_task) = &self.background_task {
            background_task.abort();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{advisory, database};
    use docs_rs_types::testing::DUMMY;
    use rustsec::VersionReq;
    use test_case::test_case;

    const OWNED_ALLOC: KrateName = KrateName::from_static("owned-alloc");

    fn assert_unmaintained(
        advisories: impl IntoIterator<Item = rustsec::Advisory>,
        expected: Option<&str>,
    ) -> Result<()> {
        let database = database(advisories)?;
        assert_eq!(
            database
                .find_unmaintained(&OWNED_ALLOC)
                .map(|advisory| advisory.id().as_str()),
            expected
        );
        Ok(())
    }

    #[test]
    fn finds_unmaintained_advisory() -> Result<()> {
        let inserted = advisory(&OWNED_ALLOC)
            .informational(Informational::Unmaintained)
            .build();

        let database = database([inserted.clone()])?;

        let queried = database
            .find_unmaintained(&OWNED_ALLOC)
            .expect("unmaintained advisory");
        assert_eq!(queried.id(), inserted.id());
        assert_eq!(queried.title(), "`owned-alloc` is unmaintained");
        assert_eq!(
            queried.metadata.informational,
            Some(Informational::Unmaintained)
        );
        Ok(())
    }

    #[test_case(Some(Informational::Unmaintained), Some("2026-09-24"), None; "withdrawn")]
    #[test_case(Some(Informational::Unmaintained), None, Some(VersionReq::parse(">= 1.0.0").unwrap()); "patched")]
    #[test_case(None, None, None; "security advisory")]
    #[test_case(Some(Informational::Unsound), None, None; "other informational")]
    #[test_case(Some(Informational::Other("future-notice".to_string())), None, None; "unknown informational")]
    fn ignores_non_matching_advisory(
        informational: Option<Informational>,
        withdrawn: Option<&str>,
        patched: Option<VersionReq>,
    ) -> Result<()> {
        assert_unmaintained(
            [advisory(&OWNED_ALLOC)
                .maybe_informational(informational)
                .maybe_withdrawn(withdrawn)
                .maybe_patched(patched)
                .build()],
            None,
        )
    }

    #[test]
    fn ignores_advisory_for_another_crate() -> Result<()> {
        let advisory_for_other_crate = advisory(&DUMMY)
            .informational(Informational::Unmaintained)
            .build();
        assert_unmaintained([advisory_for_other_crate], None)
    }

    #[test]
    fn finds_no_unmaintained_advisory_in_empty_database() -> Result<()> {
        assert_unmaintained([], None)
    }

    #[test]
    fn finds_first_unmaintained_advisory() -> Result<()> {
        let first = advisory(&OWNED_ALLOC)
            .informational(Informational::Unmaintained)
            .build();

        let database = database([
            first.clone(),
            advisory(&OWNED_ALLOC)
                .informational(Informational::Unmaintained)
                .build(),
        ])?;

        assert_eq!(
            database.find_unmaintained(&OWNED_ALLOC).unwrap().id(),
            first.id()
        );
        Ok(())
    }

    #[test]
    fn finds_unmaintained_advisory_alongside_withdrawn_advisory() -> Result<()> {
        const ID: &str = "RUSTSEC-2026-0300";

        assert_unmaintained(
            [
                advisory(&OWNED_ALLOC)
                    .informational(Informational::Unmaintained)
                    .withdrawn("2026-09-24")
                    .build(),
                advisory(&OWNED_ALLOC)
                    .id(ID)
                    .informational(Informational::Unmaintained)
                    .build(),
            ],
            Some(ID),
        )
    }
}
