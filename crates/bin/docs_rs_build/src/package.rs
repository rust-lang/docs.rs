use anyhow::{Context as _, Result, bail};
use docs_rs_crate_archive::{SourceDir, unpack_crate_archive};
use std::{
    fs::{self, File},
    path::{Path, PathBuf},
    process::Command,
};
use tracing::{debug, info, instrument};

/// Package a local crate and unpack its `.crate` archive into a temporary source directory.
#[instrument(fields(manifest_dir = %manifest_dir.display(), package))]
pub(crate) fn create(manifest_dir: &Path, package: Option<&str>) -> Result<SourceDir> {
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

    let output = command.output().context("running `cargo package`")?;
    write_cargo_output(&output.stdout);
    write_cargo_output(&output.stderr);
    if !output.status.success() {
        bail!("`cargo package` failed with {}", output.status);
    }

    let archive_path = find_single_archive(&cargo_target.join("package"))?;
    debug!(archive = %archive_path.display(), "extracting packaged crate source");
    let archive = File::open(&archive_path)
        .with_context(|| format!("opening package archive {}", archive_path.display()))?;
    let source = unpack_crate_archive(archive)
        .with_context(|| format!("extracting package archive {}", archive_path.display()))?;
    info!(source_dir = %source.path().display(), "crate archive ready");
    Ok(source)
}

fn require_package_for_virtual_workspace(manifest_path: &Path) -> Result<()> {
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

        assert!(packaged.path().join("Cargo.toml").is_file());
        assert!(packaged.path().join("src/lib.rs").is_file());
        assert!(!packaged.path().join("not-packaged").exists());
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

        let manifest = fs::read_to_string(packaged.path().join("Cargo.toml")).unwrap();
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
