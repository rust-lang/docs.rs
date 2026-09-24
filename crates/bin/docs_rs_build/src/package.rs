use anyhow::{Context as _, Result, bail};
use docs_rs_crate_archive::{SourceDir, unpack_crate_archive};
use std::{
    fs::{self, File},
    path::{Path, PathBuf},
    process::Command,
};
use tracing::{debug, info, instrument};

#[derive(Debug)]
pub(crate) struct PackagedCrate {
    pub(crate) source: SourceDir,
    pub(crate) directory_label: String,
}

/// Package a local crate and unpack its `.crate` archive into a temporary source directory.
#[instrument(fields(manifest_dir = %manifest_dir.display(), package))]
pub(crate) fn create(manifest_dir: &Path, package: Option<&str>) -> Result<PackagedCrate> {
    let temporary = tempfile::tempdir().context("creating temporary packaging directory")?;
    let cargo_target = temporary.path().join("cargo-target");
    let manifest_path = manifest_dir.join("Cargo.toml");
    if package.is_none() {
        require_package_for_virtual_workspace(&manifest_path)?;
    }

    info!("creating the crate archive with cargo package");
    let mut command = Command::new("cargo");
    command
        .args(["package", "--allow-dirty", "--no-verify"])
        .arg("--manifest-path")
        .arg(&manifest_path)
        .arg("--target-dir")
        .arg(&cargo_target)
        .current_dir(manifest_dir);
    if let Some(package) = package {
        command.args(["--package", package]);
    }

    let status = command.status().context("running `cargo package`")?;
    if !status.success() {
        bail!("`cargo package` failed with {status}");
    }

    let archive_path = find_single_archive(&cargo_target.join("package"))?;
    debug!(archive = %archive_path.display(), "extracting packaged crate source");
    let archive = File::open(&archive_path)
        .with_context(|| format!("opening package archive {}", archive_path.display()))?;
    let source = unpack_crate_archive(archive)
        .with_context(|| format!("extracting package archive {}", archive_path.display()))?;

    // Cargo removes [workspace] when packaging. Restore an empty workspace so
    // the build copy cannot accidentally join the original checkout's workspace.
    let manifest_path = source.path().join("Cargo.toml");
    let mut manifest: toml::Table = toml::from_str(&fs::read_to_string(&manifest_path)?)
        .context("parsing packaged manifest")?;
    let directory_label = directory_label(&manifest)?;
    manifest.insert("workspace".into(), toml::Value::Table(toml::Table::new()));
    fs::write(&manifest_path, toml::to_string(&manifest)?)
        .context("isolating packaged crate from parent workspaces")?;

    info!(source_dir = %source.path().display(), "crate archive ready");
    Ok(PackagedCrate {
        source,
        directory_label,
    })
}

/// Read the resolved identity from Cargo's normalized package manifest.
fn directory_label(manifest: &toml::Table) -> Result<String> {
    let package = manifest
        .get("package")
        .context("packaged manifest has no package")?;
    let name = package
        .get("name")
        .and_then(toml::Value::as_str)
        .context("packaged manifest has no package name")?;
    let version = package
        .get("version")
        .and_then(toml::Value::as_str)
        .context("packaged manifest has no package version")?;
    Ok(format!("{name}-{version}"))
}

fn require_package_for_virtual_workspace(manifest_path: &Path) -> Result<()> {
    let contents = fs::read_to_string(manifest_path)
        .with_context(|| format!("reading manifest {}", manifest_path.display()))?;
    let manifest: toml::Value = toml::from_str(&contents)
        .with_context(|| format!("parsing manifest {}", manifest_path.display()))?;
    if manifest.get("workspace").is_some() && manifest.get("package").is_none() {
        bail!(
            "`{}` is a virtual workspace; select a member with `--package <SPEC>`",
            manifest_path.display()
        );
    }
    Ok(())
}

fn find_single_archive(directory: &Path) -> Result<PathBuf> {
    let archives = fs::read_dir(directory)
        .with_context(|| format!("reading cargo package output at {}", directory.display()))?
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.path())
        .filter(|path| {
            path.extension()
                .is_some_and(|extension| extension == "crate")
        })
        .collect::<Vec<_>>();

    match archives.as_slice() {
        [archive] => Ok(archive.clone()),
        _ => bail!(
            "expected exactly one crate archive in `{}`, found {}",
            directory.display(),
            archives.len()
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_package(path: &Path, name: &str) {
        fs::create_dir_all(path.join("src")).unwrap();
        fs::write(
            path.join("Cargo.toml"),
            format!(
                r#"[package]
name = "{name}"
version = "1.2.3"
edition = "2024"
license = "MIT"
exclude = ["not-packaged"]
"#
            ),
        )
        .unwrap();
        fs::write(path.join("src/lib.rs"), "pub fn documented() {}\n").unwrap();
        fs::write(path.join("not-packaged"), "local only\n").unwrap();
    }

    #[test]
    fn packages_a_crate_instead_of_copying_its_checkout() {
        let checkout = tempfile::tempdir().unwrap();
        write_package(checkout.path(), "packaged-root");

        let packaged = create(checkout.path(), None).unwrap();

        assert!(packaged.source.path().join("Cargo.toml").is_file());
        assert!(packaged.source.path().join("src/lib.rs").is_file());
        assert!(!packaged.source.path().join("not-packaged").exists());
    }

    #[test]
    fn packaged_crate_is_independent_of_parent_workspace() {
        let checkout = tempfile::tempdir().unwrap();
        write_package(checkout.path(), "packaged-root");
        let manifest_path = checkout.path().join("Cargo.toml");
        let original = format!(
            "{}\n[workspace]\nmembers = []\n",
            fs::read_to_string(&manifest_path).unwrap()
        );
        fs::write(&manifest_path, &original).unwrap();
        let packaged = create(checkout.path(), None).unwrap();
        let nested = checkout
            .path()
            .join("target/docsrs-build/builds/release/source");
        docs_rs_rustwide::utils::copy_dir_all(packaged.source.path(), &nested, |_| {}).unwrap();

        let output = Command::new("cargo")
            .args([
                "metadata",
                "--no-deps",
                "--offline",
                "--format-version",
                "1",
            ])
            .current_dir(&nested)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let metadata: toml::Table =
            toml::from_str(&fs::read_to_string(nested.join("Cargo.toml")).unwrap()).unwrap();
        assert!(metadata["workspace"].as_table().unwrap().is_empty());
        assert_eq!(fs::read_to_string(&manifest_path).unwrap(), original);
    }

    #[test]
    fn selects_a_workspace_member() {
        let checkout = tempfile::tempdir().unwrap();
        fs::write(
            checkout.path().join("Cargo.toml"),
            "[workspace]\nmembers = [\"member\"]\nresolver = \"2\"\n",
        )
        .unwrap();
        write_package(&checkout.path().join("member"), "selected-member");

        let packaged = create(checkout.path(), Some("selected-member")).unwrap();

        assert_eq!(packaged.directory_label, "selected-member-1.2.3");
        let manifest = fs::read_to_string(packaged.source.path().join("Cargo.toml")).unwrap();
        assert!(manifest.contains("name = \"selected-member\""));
    }

    #[test]
    fn virtual_workspace_requires_a_package() {
        let checkout = tempfile::tempdir().unwrap();
        fs::write(
            checkout.path().join("Cargo.toml"),
            "[workspace]\nmembers = [\"member\"]\nresolver = \"2\"\n",
        )
        .unwrap();
        write_package(&checkout.path().join("member"), "workspace-member");

        let error = create(checkout.path(), None).unwrap_err();

        assert!(error.to_string().contains("virtual workspace"));
        assert!(error.to_string().contains("--package"));
    }
}
