use anyhow::{Context as _, Result, bail};
use std::{
    fs::{self, File, OpenOptions},
    path::Path,
};

/// Separate from Rustwide's short-lived initialization lock; never unlink this file.
pub(crate) struct WorkspaceLock {
    _file: File,
}

impl WorkspaceLock {
    pub(crate) fn acquire(path: &Path, wait: bool) -> Result<Self> {
        fs::create_dir_all(path)
            .with_context(|| format!("creating workspace {}", path.display()))?;
        let lock_path = path.join(".docsrs-workspace.lock");
        let file = OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .truncate(false)
            .open(&lock_path)
            .with_context(|| format!("opening workspace lock {}", lock_path.display()))?;
        if wait {
            tracing::info!(workspace = %path.display(), "waiting for workspace lock");
            file.lock()
                .with_context(|| format!("locking workspace {}", path.display()))?;
        } else {
            match file.try_lock() {
                Ok(()) => {}
                Err(std::fs::TryLockError::WouldBlock) => bail!(
                    "workspace {} is already in use; stop its current owner or select a different workspace",
                    path.display()
                ),
                Err(std::fs::TryLockError::Error(error)) => {
                    return Err(error)
                        .with_context(|| format!("locking workspace {}", path.display()));
                }
            }
        }
        tracing::debug!(workspace = %path.display(), "acquired workspace lock");
        Ok(Self { _file: file })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{process::Command, sync::mpsc, time::Duration};

    #[test]
    fn competing_environment_fails_before_initialization() -> Result<()> {
        let directory = tempfile::tempdir()?;
        let _lock = WorkspaceLock::acquire(directory.path(), false)?;
        let error = crate::BuildEnvironment::builder(directory.path())
            .build()
            .err()
            .expect("a competing environment must fail");
        assert!(error.to_string().contains("already in use"));
        assert!(!directory.path().join("builds").exists());
        Ok(())
    }

    #[test]
    fn waiting_owner_proceeds_after_release() -> Result<()> {
        let directory = tempfile::tempdir()?;
        let lock = WorkspaceLock::acquire(directory.path(), false)?;
        let path = directory.path().to_owned();
        let (send, receive) = mpsc::channel();
        let waiter = std::thread::spawn(move || {
            let lock = WorkspaceLock::acquire(&path, true).unwrap();
            send.send(()).unwrap();
            drop(lock);
        });
        assert!(matches!(
            receive.recv_timeout(Duration::from_millis(50)),
            Err(mpsc::RecvTimeoutError::Timeout)
        ));
        drop(lock);
        receive.recv_timeout(Duration::from_secs(5))?;
        waiter.join().unwrap();
        Ok(())
    }

    #[test]
    fn lock_coordinates_processes() -> Result<()> {
        const CHILD_PATH: &str = "DOCSRS_WORKSPACE_LOCK_TEST_PATH";
        if let Some(path) = std::env::var_os(CHILD_PATH) {
            assert!(WorkspaceLock::acquire(Path::new(&path), false).is_err());
            return Ok(());
        }
        let directory = tempfile::tempdir()?;
        let lock = WorkspaceLock::acquire(directory.path(), false)?;
        let output = Command::new(std::env::current_exe()?)
            .args([
                "--exact",
                "workspace_lock::tests::lock_coordinates_processes",
            ])
            .env(CHILD_PATH, directory.path())
            .output()?;
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stdout)
        );
        drop(lock);
        let _lock = WorkspaceLock::acquire(directory.path(), false)?;
        Ok(())
    }
}
