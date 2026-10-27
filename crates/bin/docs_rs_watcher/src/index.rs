use crate::Config;
use anyhow::{Context as _, Result};
use crates_index_diff::{Change, gix, index::diff::Order};
use docs_rs_database::service_config::{ConfigName, get_config, set_config};
use docs_rs_utils::run_blocking;
use sqlx::{Connection as _, PgConnection, Postgres, Transaction};
use std::{
    path::{Path, PathBuf},
    process::Command,
    sync::{Arc, Mutex, atomic::AtomicBool},
};
use tracing::{error, warn};

const THREAD_NAME: &str = "registry-index";
const DEFAULT_REGISTRY_URL: &str = "https://github.com/rust-lang/crates.io-index";

#[allow(clippy::disallowed_types)]
pub type FullIndex = crates_index::GitIndex;

#[allow(clippy::disallowed_types)]
pub type DiffIndex = crates_index_diff::Index;

#[derive(Default)]
struct GitIndexes {
    path: PathBuf,
    repository_url: Option<String>,
    diff: Option<DiffIndex>,
    full: Option<FullIndex>,
}

/// Registry access for one complete operation, including database and storage repairs.
///
/// Provides:
/// * async operations
/// * cross-container / server synchronization between watcher & consistency check
///
/// Construction acquires the registry lock. Git handles are opened lazily and
/// share one repository, but keep independent diff and full-index read positions.
/// Keep this object alive until all repairs complete, then commit the checkpoint
/// transaction. Dropping it rolls back the checkpoint transaction.
#[must_use = "keep the index alive until the registry operation completes"]
pub struct Index<'conn> {
    /// transaction holding the lock.
    /// Will be used to get / set the last-seen-ref too.
    /// Can, but doesn't have to be used by other things.
    lock_transaction: Transaction<'conn, Postgres>,
    // Mutex only locked inside run_blocking; never held across an await point.
    git: Arc<Mutex<GitIndexes>>,
}

impl GitIndexes {
    fn diff(&mut self) -> Result<&mut DiffIndex> {
        if self.diff.is_none() {
            let clone_options = crates_index_diff::index::CloneOptions {
                url: self
                    .repository_url
                    .as_deref()
                    .unwrap_or(DEFAULT_REGISTRY_URL)
                    .to_string(),
            };

            self.diff = Some(
                DiffIndex::from_path_or_cloned_with_options(
                    &self.path,
                    gix::progress::Discard,
                    &AtomicBool::default(),
                    clone_options,
                )
                .context("initialising registry diff index")?,
            );
        }
        Ok(self.diff.as_mut().unwrap())
    }

    fn full(&mut self) -> Result<&mut FullIndex> {
        if self.full.is_none() {
            self.full = Some(
                FullIndex::with_path(
                    &self.path,
                    self.repository_url
                        .as_deref()
                        .unwrap_or(DEFAULT_REGISTRY_URL),
                )
                .context("initialising registry full index")?,
            );
        }

        Ok(self.full.as_mut().unwrap())
    }
}

impl<'conn> Index<'conn> {
    pub async fn from_config(config: &Config, conn: &'conn mut PgConnection) -> Result<Self> {
        Self::from_url(
            &config.registry_index_path,
            config.registry_url.as_deref(),
            conn,
        )
        .await
    }

    pub async fn from_url(
        path: impl AsRef<Path>,
        repository_url: Option<impl AsRef<str>>,
        conn: &'conn mut PgConnection,
    ) -> Result<Self> {
        let mut transaction = conn.begin().await?;
        // akquire a postgres advisory lock. Unique key are the parameters,
        // so the schema (`public` on prod, different in tests),
        // and "docs_rs_registry".
        // The lock will be held until the transaction ends (commit or rollback).
        sqlx::query!(
            "SELECT
                pg_advisory_xact_lock(
                    hashtext(current_schema()),
                    hashtext('docs_rs_registry')
                )
            ",
        )
        .execute(&mut *transaction)
        .await?;

        let path = path.as_ref().to_path_buf();
        let repository_url = repository_url.map(|url| url.as_ref().to_owned());

        Ok(Self {
            lock_transaction: transaction,
            git: Arc::new(Mutex::new(GitIndexes {
                path,
                repository_url,
                ..Default::default()
            })),
        })
    }

    pub async fn new(path: impl AsRef<Path>, conn: &'conn mut PgConnection) -> Result<Self> {
        Self::from_url(path, None::<&str>, conn).await
    }

    /// our transaction with the lock.
    /// Use it for changes when you want to commit them only when
    /// no errors happened.
    pub(crate) fn connection(&mut self) -> &mut PgConnection {
        &mut self.lock_transaction
    }

    pub async fn last_seen_reference(&mut self) -> Result<Option<gix::ObjectId>> {
        get_config::<String>(self.connection(), ConfigName::LastSeenIndexReference)
            .await?
            .map(|value| gix::ObjectId::from_hex(value.as_bytes()).map_err(Into::into))
            .transpose()
    }

    pub async fn set_last_seen_reference(&mut self, oid: gix::ObjectId) -> Result<()> {
        set_config(
            self.connection(),
            ConfigName::LastSeenIndexReference,
            oid.to_string(),
        )
        .await?;
        Ok(())
    }

    /// commit the changes, unlock the index.
    pub async fn commit(self) -> Result<()> {
        self.lock_transaction.commit().await?;
        Ok(())
    }

    async fn with_git_indexes<T, F>(&mut self, f: F) -> Result<T>
    where
        T: Send + 'static,
        F: FnOnce(&mut GitIndexes) -> Result<T> + Send + 'static,
    {
        let git = self.git.clone();
        run_blocking(THREAD_NAME, move || f(&mut git.lock().unwrap())).await
    }

    pub async fn peek_changes_ordered(&mut self) -> Result<(Vec<Change>, gix::ObjectId)> {
        let reference = if let Some(reference) = self.last_seen_reference().await? {
            reference
        } else {
            warn!("No last-seen reference found; starting at the latest index commit");
            self.latest_commit_reference().await?
        };

        self.with_git_indexes(move |git_indexes| {
            let diff = git_indexes.diff()?;

            // move the last-seen-ref pointer on the local index clone
            // aka: the "starting ref" for this diff.
            diff.set_last_seen_reference(reference)?;

            diff.peek_changes_with_options(
                gix::progress::Discard,
                &AtomicBool::default(),
                Order::AsInCratesIndex,
            )
            .map_err(Into::into)
        })
        .await
    }

    /// fetch the latest commit in the registry index.
    pub async fn latest_commit_reference(&mut self) -> Result<gix::ObjectId> {
        self.with_full_index_at(None, |full_index| Ok((*full_index.commit()).into()))
            .await
    }

    /// Read a full-index snapshot, fetching only when the supplied commit is
    /// unavailable locally. Without a reference, fetch and read HEAD.
    /// Selecting a snapshot changes only the full handle's in-memory position;
    /// it does not change the diff cursor or the database checkpoint.
    pub(crate) async fn with_full_index_at<T, F>(
        &mut self,
        reference: Option<gix::ObjectId>,
        f: F,
    ) -> Result<T>
    where
        T: Send + 'static,
        F: FnOnce(&FullIndex) -> Result<T> + Send + 'static,
    {
        self.with_git_indexes(move |git_indexes| {
            let full_index = git_indexes.full()?;
            if let Some(reference) = reference {
                let reference = reference.to_string();
                // first check if the wanted ref is already in our local repo clone.
                // Then we don't need to `.update`
                if full_index.set_commit_from_refspec(&reference).is_err() {
                    // it we don't find, it update and try again.
                    full_index.update()?;
                    full_index
                        .set_commit_from_refspec(&reference)
                        .with_context(|| {
                            format!("Opening registry index at reference {reference}")
                        })?;
                }
            } else {
                full_index.update()?;
            }

            f(full_index)
        })
        .await
    }

    pub async fn run_git_gc(&mut self) {
        if let Err(err) = self
            .with_git_indexes(|git| {
                Command::new("git")
                    .arg("-C")
                    .arg(&git.path)
                    .args(["gc", "--auto"])
                    .output()
                    .with_context(|| format!("running git gc --auto in {}", git.path.display()))?;
                Ok(())
            })
            .await
        {
            error!(?err, "failed to run `git gc --auto`");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        testing::{TestEnvironment, registry_history},
        watch_registry,
    };
    use crates_index_diff::gix::ObjectId;
    use docs_rs_types::testing::{KRATE, V2};
    use std::time::Duration;
    use tokio::time::{sleep, timeout};

    #[test_case::test_case(false; "no checkpoint")]
    #[test_case::test_case(true; "existing checkpoint")]
    #[tokio::test(flavor = "multi_thread")]
    async fn latest_commit_uses_full_handle(checkpoint_exists: bool) -> Result<()> {
        let env = TestEnvironment::new().await?;
        let (config, old_reference, expected_head) = registry_history(&env).await?;
        let mut conn = env.async_conn().await?;

        // open the testing git index, checkpoint will be set after the first commit.
        let mut index = Index::from_config(&config, &mut conn).await?;
        let checkpoint = checkpoint_exists.then_some(old_reference);
        if let Some(checkpoint) = checkpoint {
            index.set_last_seen_reference(checkpoint).await?;
        }

        assert_eq!(index.latest_commit_reference().await?, expected_head);
        assert!(index.git.lock().unwrap().diff.is_none());
        assert!(index.git.lock().unwrap().full.is_some());

        // A previously selected full snapshot must not make HEAD reads stale.
        index
            .with_full_index_at(Some(old_reference), |_| Ok(()))
            .await?;
        assert_eq!(index.latest_commit_reference().await?, expected_head);
        assert!(index.git.lock().unwrap().diff.is_none());
        assert_eq!(index.last_seen_reference().await?, checkpoint);

        let (changes, head) = index.peek_changes_ordered().await?;
        assert_eq!(head, expected_head);
        assert_eq!(changes.len(), if checkpoint_exists { 2 } else { 0 });
        index.commit().await?;
        Ok(())
    }

    #[test_case::test_case(false; "diff first")]
    #[test_case::test_case(true; "full first")]
    #[tokio::test(flavor = "multi_thread")]
    async fn both_git_handles_keep_independent_positions(full_first: bool) -> Result<()> {
        let env = TestEnvironment::new().await?;
        let (config, checkpoint, expected_head) = registry_history(&env).await?;
        let mut conn = env.async_conn().await?;
        let mut index = Index::from_config(&config, &mut conn).await?;
        assert!(index.git.lock().unwrap().diff.is_none());
        assert!(index.git.lock().unwrap().full.is_none());
        assert!(!config.registry_index_path.exists());
        index.set_last_seen_reference(checkpoint).await?;
        // Checkpoint-only access does not open either Git handle.
        assert!(index.git.lock().unwrap().diff.is_none());
        assert!(index.git.lock().unwrap().full.is_none());

        fn snapshot(full: &FullIndex) -> Result<(usize, bool)> {
            let krate = full.crate_("krate").context("missing fixture crate")?;
            Ok((krate.versions().len(), krate.versions()[0].is_yanked()))
        }

        if full_first {
            assert_eq!(
                index.with_full_index_at(Some(checkpoint), snapshot).await?,
                (1, true)
            );
            assert!(index.git.lock().unwrap().diff.is_none());
        }

        let (changes, head) = index.peek_changes_ordered().await?;
        assert_eq!(head, expected_head);
        assert_eq!(changes.len(), 2);

        if !full_first {
            assert!(index.git.lock().unwrap().full.is_none());
        }

        assert_eq!(
            index.with_full_index_at(Some(checkpoint), snapshot).await?,
            (1, true)
        );
        // Selecting an old full snapshot does not move the diff starting point.
        let (again, head) = index.peek_changes_ordered().await?;
        assert_eq!(head, expected_head);
        assert_eq!(again.len(), changes.len());

        // A later HEAD read resets the full handle's previous snapshot selection.
        assert_eq!(index.with_full_index_at(None, snapshot).await?, (2, false));
        assert_eq!(index.last_seen_reference().await?, Some(checkpoint));
        assert!(index.git.lock().unwrap().diff.is_some());
        assert!(index.git.lock().unwrap().full.is_some());

        index.commit().await?;
        Ok(())
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn full_snapshot_fetches_missing_commit_but_reuses_local_commit() -> Result<()> {
        let env = TestEnvironment::new().await?;
        let (config, _, old_head) = registry_history(&env).await?;
        let mut conn = env.async_conn().await?;
        let mut index = Index::from_config(&config, &mut conn).await?;
        index.with_full_index_at(Some(old_head), |_| Ok(())).await?;

        let registry = env.test_registry();
        registry.mock_index_missing(&KRATE).await;
        let new_head = registry.git_head().await?;
        assert_ne!(new_head, old_head);
        // This commit was created after the local clone; resolving it requires a fetch.
        index
            .with_full_index_at(Some(new_head), |full| {
                assert!(full.crate_(KRATE.as_str()).is_none());
                Ok(())
            })
            .await?;

        // Make the remote unavailable: existing snapshots must remain readable.
        let origin = registry.enable_git_index().await?.to_file_path().unwrap();
        std::fs::rename(&origin, origin.with_file_name("offline-index"))?;
        index
            .with_full_index_at(Some(old_head), |full| {
                assert!(full.crate_(KRATE.as_str()).is_some());
                Ok(())
            })
            .await?;
        index.commit().await?;
        Ok(())
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn checkpoint_changes_wait_and_lock_is_released_on_drop() -> Result<()> {
        let env = TestEnvironment::new().await?;
        let mut conn = env.async_conn().await?;
        let mut lock_conn = env.async_conn().await?;
        let mut index = Index::from_config(env.config(), &mut lock_conn).await?;
        let reference = ObjectId::from_hex(&[b'1'; 40])?;
        assert!(
            timeout(
                Duration::from_millis(100),
                Index::from_config(env.config(), &mut conn)
            )
            .await
            .is_err()
        );
        assert_eq!(index.last_seen_reference().await?, None);

        // Error returns and cancellation drop both the transaction and its
        // pooled connection, which flushes SQLx's queued rollback.
        drop(index);
        drop(lock_conn);
        timeout(Duration::from_secs(5), async {
            let mut index = Index::from_config(env.config(), &mut conn).await?;
            index.set_last_seen_reference(reference).await?;
            index.commit().await
        })
        .await??;
        let mut index = Index::from_config(env.config(), &mut conn).await?;
        assert_eq!(index.last_seen_reference().await?, Some(reference));
        index.commit().await?;
        Ok(())
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn watcher_waits_for_registry_lock_then_catches_up() -> Result<()> {
        let env = TestEnvironment::new().await?;
        let (config, checkpoint, expected_head) = registry_history(&env).await?;
        let mut conn = env.async_conn().await?;
        let mut index = Index::from_config(env.config(), &mut conn).await?;
        index.set_last_seen_reference(checkpoint).await?;
        index.commit().await?;
        let mut lock_conn = env.async_conn().await?;
        let mut index = Index::from_config(env.config(), &mut lock_conn).await?;

        assert!(
            timeout(Duration::from_millis(100), watch_registry(&config, &env))
                .await
                .is_err()
        );
        assert_eq!(index.last_seen_reference().await?, Some(checkpoint));
        assert!(env.build_queue()?.queued_crates().await?.is_empty());
        index.commit().await?;

        timeout(Duration::from_secs(10), async {
            tokio::select! {
                result = watch_registry(&config, &env) => result,
                result = async {
                    loop {
                        let mut index = Index::from_config(env.config(), &mut conn).await?;
                        let reference = index.last_seen_reference().await?;
                        index.commit().await?;
                        if reference == Some(expected_head) {
                            break;
                        }
                        sleep(Duration::from_millis(10)).await;
                    }
                    Ok(())
                } => result,
            }
        })
        .await??;
        assert!(env.build_queue()?.has_build_queued(&KRATE, &V2,).await?);
        Ok(())
    }
    #[tokio::test(flavor = "multi_thread")]
    async fn test_last_seen_reference_in_db() -> Result<()> {
        let env = TestEnvironment::new().await?;

        let mut conn = env.async_conn().await?;
        let queue = env.build_queue()?;
        queue.unlock().await?;
        assert!(!queue.is_locked().await?);

        let mut index = Index::from_config(env.config(), &mut conn).await?;
        // initial db ref is empty
        assert_eq!(index.last_seen_reference().await?, None);
        assert!(!queue.is_locked().await?);

        let oid = ObjectId::from_hex(b"ffffffffffffffffffffffffffffffffffffffff")?;
        index.set_last_seen_reference(oid).await?;
        assert_eq!(index.last_seen_reference().await?, Some(oid));
        index.commit().await?;
        assert!(!queue.is_locked().await?);

        Ok(())
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_broken_db_reference_breaks() -> Result<()> {
        let env = TestEnvironment::new().await?;

        let mut conn = env.async_conn().await?;
        set_config(&mut conn, ConfigName::LastSeenIndexReference, "invalid")
            .await
            .unwrap();

        let mut index = Index::from_config(env.config(), &mut conn).await?;
        assert!(index.last_seen_reference().await.is_err());
        index.commit().await?;

        Ok(())
    }
}
