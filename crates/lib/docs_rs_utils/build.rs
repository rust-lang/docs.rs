use anyhow::{Context as _, Result, anyhow};
use std::{env, path::PathBuf, process::Command};

fn main() -> Result<()> {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-env-changed=GIT_SHA");
    read_git_version()?;
    Ok(())
}

fn read_git_version() -> Result<()> {
    if let Ok(v) = env::var("GIT_SHA") {
        // first try to read an externally provided git SHA, e.g., from CI
        println!("cargo:rustc-env=GIT_SHA={v}");
    } else {
        if let Some(path) = git_head_ref_path().context("error trying to get git head ref path")? {
            println!("cargo:rerun-if-changed={}", path.display());
        }

        // then try to read the git repo.
        let maybe_hash = match git_output(["rev-parse", "--short", "HEAD"]) {
            Ok(hash) => Some(hash),
            Err(err) => {
                eprintln!("error trying to get git head ref path: {:?}", err);
                None
            }
        };

        let git_hash = maybe_hash.as_deref().unwrap_or("???????");
        println!("cargo:rustc-env=GIT_SHA={git_hash}");
    }

    println!(
        "cargo:rustc-env=BUILD_DATE={}",
        time::OffsetDateTime::now_utc().date(),
    );

    Ok(())
}

fn git_head_ref_path() -> Result<Option<PathBuf>> {
    let git_dir = git_output(["rev-parse", "--git-dir"])?;
    let output = Command::new("git")
        .args(["symbolic-ref", "-q", "HEAD"])
        .output()?;

    if output.status.success() {
        let head_ref = String::from_utf8_lossy(&output.stdout).trim().to_string();
        Ok(Some(PathBuf::from(git_dir).join(head_ref)))
    } else {
        Ok(None)
    }
}

fn git_output<const N: usize>(args: [&str; N]) -> Result<String> {
    let output = Command::new("git").args(args).output()?;

    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
    } else {
        Err(
            anyhow!(String::from_utf8_lossy(&output.stderr).trim().to_string())
                .context(format!("error running git command: {:?}", args)),
        )
    }
}
