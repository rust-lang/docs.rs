use anyhow::{Context as _, Result, bail};
use flate2::read::GzDecoder;
use std::{
    fs::{self, File},
    path::{Path, PathBuf},
    process::Command,
};
use tempfile::TempDir;
use tracing::{debug, info, instrument};

/// A `cargo package` archive extracted into a temporary source directory.
///
/// Keeping this value alive keeps the source directory alive.
#[derive(Debug)]
pub(crate) struct PackagedCrate {
    _temporary: TempDir,
    source_dir: PathBuf,
}

impl PackagedCrate {
    #[instrument(fields(manifest_dir = %manifest_dir.display(), package))]
    pub(crate) fn create(manifest_dir: &Path, package: Option<&str>) -> Result<Self> {
        let temporary = tempfile::tempdir().context("creating temporary packaging directory")?;
        let cargo_target = temporary.path().join("cargo-target");
        let manifest_path = manifest_dir.join("Cargo.toml");
        require_package_for_virtual_workspace(&manifest_path, package)?;

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

        let output = command.output().context("running `cargo package`")?;
        write_cargo_output(&output.stdout);
        write_cargo_output(&output.stderr);
        if !output.status.success() {
            bail!("`cargo package` failed with {}", output.status);
        }

        let archive_path = find_single_archive(&cargo_target.join("package"))?;
        debug!(archive = %archive_path.display(), "extracting packaged crate source");
        let unpacked = temporary.path().join("source");
        fs::create_dir(&unpacked)?;
        let decoder = GzDecoder::new(
            File::open(&archive_path)
                .with_context(|| format!("opening package archive {}", archive_path.display()))?,
        );
        tar::Archive::new(decoder)
            .unpack(&unpacked)
            .with_context(|| format!("extracting package archive {}", archive_path.display()))?;
        let source_dir = find_single_directory(&unpacked)?;
        info!(source_dir = %source_dir.display(), "crate archive ready");

        Ok(Self {
            _temporary: temporary,
            source_dir,
        })
    }

    pub(crate) fn source_dir(&self) -> &Path {
        &self.source_dir
    }
}

fn require_package_for_virtual_workspace(
    manifest_path: &Path,
    package: Option<&str>,
) -> Result<()> {
    if package.is_some() {
        return Ok(());
    }

    let contents = fs::read_to_string(manifest_path)
        .with_context(|| format!("reading manifest {}", manifest_path.display()))?;
    let manifest: toml::Value = toml::from_str(&contents)
        .with_context(|| format!("parsing manifest {}", manifest_path.display()))?;
    if manifest.get("workspace").is_some() && manifest.get("package").is_none() {
        bail!(
            "{} is a virtual workspace; select a member with `--package <SPEC>`",
            manifest_path.display()
        );
    }
    Ok(())
}

fn write_cargo_output(output: &[u8]) {
    if !output.is_empty() {
        print!("{}", String::from_utf8_lossy(output));
    }
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
            "expected exactly one crate archive in {}, found {}",
            directory.display(),
            archives.len()
        ),
    }
}

fn find_single_directory(directory: &Path) -> Result<PathBuf> {
    let entries = fs::read_dir(directory)
        .with_context(|| format!("reading extracted package at {}", directory.display()))?
        .collect::<Result<Vec<_>, _>>()?;

    match entries.as_slice() {
        [entry] if entry.file_type()?.is_dir() => Ok(entry.path()),
        _ => bail!(
            "expected the crate archive to contain one root directory, found {} entries",
            entries.len()
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

        let packaged = PackagedCrate::create(checkout.path(), None).unwrap();

        assert!(packaged.source_dir().join("Cargo.toml").is_file());
        assert!(packaged.source_dir().join("src/lib.rs").is_file());
        assert!(!packaged.source_dir().join("not-packaged").exists());
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

        let packaged = PackagedCrate::create(checkout.path(), Some("selected-member")).unwrap();

        let manifest = fs::read_to_string(packaged.source_dir().join("Cargo.toml")).unwrap();
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

        let error = PackagedCrate::create(checkout.path(), None).unwrap_err();

        assert!(error.to_string().contains("virtual workspace"));
        assert!(error.to_string().contains("--package"));
    }
}
