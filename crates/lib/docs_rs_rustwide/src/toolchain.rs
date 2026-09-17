use anyhow::{Context as _, Result, anyhow, bail};
use docs_rs_utils::retry;
use docsrs_metadata::{DEFAULT_TARGETS, HOST_TARGET};
use rustwide::{Toolchain, Workspace, cmd::Command, toolchain::ToolchainError};
use std::{
    collections::HashSet,
    time::{Duration, Instant},
};
use tracing::{debug, instrument, warn};

pub(crate) const DEFAULT_TOOLCHAIN_UPDATE_INTERVAL: Duration = Duration::from_secs(60 * 60);
const TOOLCHAIN_COMPONENTS: &[&str] = &["llvm-tools-preview", "rustc-dev", "rustfmt"];

/// Toolchain lifecycle state. The workspace is supplied per operation because it can be recreated.
pub(crate) struct ManagedToolchain {
    toolchain: Toolchain,
    update_interval: Duration,
    last_update_check: Option<Instant>,
}

impl ManagedToolchain {
    pub(crate) fn new(toolchain: Toolchain, update_interval: Duration) -> Self {
        Self {
            toolchain,
            update_interval,
            last_update_check: None,
        }
    }

    pub(crate) fn get(&self) -> &Toolchain {
        &self.toolchain
    }

    pub(crate) fn select(&mut self, toolchain: Toolchain) -> bool {
        let changed = self.toolchain != toolchain;
        self.toolchain = toolchain;
        if changed {
            self.last_update_check = None;
        }
        changed
    }

    pub(crate) fn update_due(&self, now: Instant) -> bool {
        self.last_update_check
            .is_none_or(|last| now.duration_since(last) >= self.update_interval)
    }

    // Called by BuildEnvironment only after any required cache purge succeeds.
    pub(crate) fn mark_updated(&mut self, now: Instant) {
        self.last_update_check = Some(now);
    }

    pub(crate) fn resource_suffix(&self, workspace: &Workspace) -> Result<String> {
        Ok(format!(
            "-{}",
            parse_rustc_version(&self.rustc_version(workspace)?)?
        ))
    }

    fn is_toolchain_installed(&self, workspace: &Workspace) -> Result<bool> {
        if self.toolchain.as_dist().is_some() {
            return match self.toolchain.installed_targets(workspace) {
                Ok(_) => Ok(true),
                Err(error)
                    if matches!(
                        error.downcast_ref::<ToolchainError>(),
                        Some(ToolchainError::NotInstalled)
                    ) =>
                {
                    Ok(false)
                }
                Err(error) => Err(error),
            };
        }

        Ok(workspace.installed_toolchains()?.contains(&self.toolchain))
    }

    #[instrument(skip_all)]
    fn ensure_toolchain_installed(&self, workspace: &Workspace) -> Result<bool> {
        if self.is_toolchain_installed(workspace)? {
            debug!("toolchain is already installed");
            return Ok(false);
        }

        debug!("installing toolchain");
        retry(|| self.toolchain.install(workspace), 3)?;
        debug!("toolchain installed");
        Ok(true)
    }

    // Establish the toolchain invariant for this environment without checking
    // whether an installed distribution toolchain can be updated. Unmanaged
    // targets are preserved here and only cleaned up by `update`.
    #[instrument(skip_all)]
    pub(crate) fn ensure_ready(&self, workspace: &Workspace) -> Result<bool> {
        let installed = self.ensure_toolchain_installed(workspace)?;

        if self.toolchain.as_ci().is_none() {
            let installed_targets = self.toolchain.installed_targets(workspace)?;
            self.ensure_required_toolchain_targets(workspace, &installed_targets)?;
            self.ensure_toolchain_components(workspace);
        }

        debug!(installed, "toolchain is ready");
        Ok(installed)
    }

    pub(crate) fn update(&self, workspace: &Workspace) -> Result<bool> {
        if self.toolchain.as_ci().is_some() {
            debug!("reinstalling CI toolchain");
            retry(|| self.toolchain.install(workspace), 3)?;
            return Ok(true);
        }

        // Version detection is allowed to fail when the toolchain is not installed yet.
        let old_version = self.rustc_version(workspace).ok();
        let installed_targets = match self.toolchain.installed_targets(workspace) {
            Ok(targets) => targets,
            Err(error)
                if matches!(
                    error.downcast_ref::<ToolchainError>(),
                    Some(ToolchainError::NotInstalled)
                ) =>
            {
                Vec::new()
            }
            Err(error) => return Err(error),
        };

        // Remove no-longer-managed targets before updating. Otherwise rustup can
        // refuse an update when one of those targets disappeared upstream.
        let managed_targets = Self::managed_toolchain_targets();
        for target in &installed_targets {
            if !managed_targets.contains(target) {
                debug!(target, "removing unmanaged target before toolchain update");
                retry(|| self.toolchain.remove_target(workspace, target), 3)?;
            }
        }

        debug!(old_version, "installing or updating toolchain");
        retry(|| self.toolchain.install(workspace), 3)?;
        self.ensure_ready(workspace)?;

        let new_version = self.rustc_version(workspace)?;
        let changed = old_version.as_ref() != Some(&new_version);
        debug!(changed, new_version, "toolchain update complete");
        Ok(changed)
    }

    pub(crate) fn rustc_version(&self, workspace: &Workspace) -> Result<String> {
        if let Some(ci) = self.toolchain.as_ci() {
            let version = ci_rustc_version(ci.sha());
            debug!(version, "using synthetic CI rustc version");
            return Ok(version);
        }

        debug!("detecting rustc version");
        let output = Command::new(workspace, self.toolchain.rustc())
            .arg("--version")
            .log_output(false)
            .run_capture()?;
        let [version] = output.stdout_lines() else {
            bail!("invalid output returned by `rustc --version`");
        };
        debug!(version, "detected rustc version");
        Ok(version.clone())
    }

    #[instrument(skip_all)]
    pub(crate) fn ensure_target_installed(
        &self,
        workspace: &Workspace,
        target: impl AsRef<str>,
    ) -> Result<()> {
        let target = target.as_ref();
        debug!("ensuring target is installed");
        self.get()
            .add_target(workspace, target)
            .context("error adding non-default target to toolchain")?;

        Ok(())
    }

    fn ensure_toolchain_components(&self, workspace: &Workspace) {
        for component in TOOLCHAIN_COMPONENTS {
            debug!(component, "ensuring toolchain component is installed");
            if let Err(error) = self.toolchain.add_component(workspace, component) {
                // A newly published nightly can temporarily lack a component. Builds
                // that do not need it should still be allowed to proceed.
                warn!("failed to install toolchain component {component}: {error}");
            }
        }
    }

    fn managed_toolchain_targets() -> HashSet<String> {
        DEFAULT_TARGETS
            .iter()
            .chain([&HOST_TARGET])
            .map(|target| (*target).to_owned())
            .collect()
    }

    fn ensure_required_toolchain_targets(
        &self,
        workspace: &Workspace,
        installed_targets: &[String],
    ) -> Result<()> {
        let mut targets_to_install = Self::managed_toolchain_targets();

        for target in installed_targets {
            targets_to_install.remove(target);
        }
        for target in targets_to_install {
            debug!(target, "installing required toolchain target");
            retry(|| self.toolchain.add_target(workspace, &target), 3)?;
        }
        Ok(())
    }
}

fn ci_rustc_version(sha: &str) -> String {
    format!("rustc 1.9999.0-nightly ({sha} 2999-12-29)")
}

fn parse_rustc_version(version: &str) -> Result<String> {
    let mut outer = version.splitn(3, ' ');
    let _binary = outer.next();
    let release = outer
        .next()
        .ok_or_else(|| anyhow!("missing release in rustc version `{version}`"))?;
    let details = outer
        .next()
        .and_then(|value| value.strip_prefix('('))
        .and_then(|value| value.strip_suffix(')'))
        .ok_or_else(|| anyhow!("missing details in rustc version `{version}`"))?;
    let mut details = details.split_whitespace();
    let commit = details
        .next()
        .ok_or_else(|| anyhow!("missing commit in rustc version `{version}`"))?;
    let date = details
        .next()
        .ok_or_else(|| anyhow!("missing date in rustc version `{version}`"))?;
    Ok(format!("{}-{release}-{commit}", date.replace('-', "")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maintenance_schedule_and_selection_changes() {
        let interval = Duration::from_secs(60);
        let mut toolchain = ManagedToolchain::new(Toolchain::dist("nightly"), interval);
        let now = Instant::now();
        assert!(toolchain.update_due(now));
        toolchain.mark_updated(now);
        assert!(!toolchain.update_due(now + interval - Duration::from_nanos(1)));
        assert!(toolchain.update_due(now + interval));
        assert!(!toolchain.select(Toolchain::dist("nightly")));
        assert!(!toolchain.update_due(now));
        assert!(toolchain.select(Toolchain::dist("stable")));
        assert!(toolchain.update_due(now));
        assert_eq!(toolchain.get(), &Toolchain::dist("stable"));
    }

    #[test]
    fn zero_interval_always_checks_for_updates() {
        let mut toolchain = ManagedToolchain::new(Toolchain::dist("nightly"), Duration::ZERO);
        let now = Instant::now();
        toolchain.mark_updated(now);
        assert!(toolchain.update_due(now));
    }

    #[test]
    fn parses_rustc_resource_version() {
        assert_eq!(
            parse_rustc_version("rustc 1.10.0-nightly (57ef01513 2016-05-23)").unwrap(),
            "20160523-1.10.0-nightly-57ef01513"
        );
    }

    #[test]
    fn creates_ci_rustc_resource_version() {
        assert_eq!(
            parse_rustc_version(&ci_rustc_version("0123456789abcdef")).unwrap(),
            "29991229-1.9999.0-nightly-0123456789abcdef"
        );
    }
}
