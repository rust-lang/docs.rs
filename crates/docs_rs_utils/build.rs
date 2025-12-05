use anyhow::Result;
use std::env;

fn main() -> Result<()> {
    read_git_version()?;
    Ok(())
}

fn read_git_version() -> Result<()> {
    if let Ok(v) = env::var("GIT_SHA") {
        // first try to read an externally provided git SAH, e.g., from CI
        println!("cargo:rustc-env=GIT_SHA={v}");
    } else {
        // then try to read the git repo.
        let maybe_hash = get_git_hash()?;
        let git_hash = maybe_hash.as_deref().unwrap_or("???????");
        println!("cargo:rustc-env=GIT_SHA={git_hash}");
    }

    println!(
        "cargo:rustc-env=BUILD_DATE={}",
        time::OffsetDateTime::now_utc().date(),
    );

    Ok(())
}

fn get_git_hash() -> Result<Option<String>> {
    use std::process::Command;

    let output = Command::new("git")
        .args(["rev-parse", "--short", "HEAD"])
        .output();

    match output {
        Ok(output) if output.status.success() => {
            let hash = String::from_utf8(output.stdout)?.trim().to_string();

            Ok(Some(hash))
        }
        Ok(output) => {
            let err = String::from_utf8_lossy(&output.stderr);
            eprintln!("failed to get git repo: {}", err.trim());
            Ok(None)
        }
        Err(err) => {
            eprintln!("failed to execute git: {err}");
            Ok(None)
        }
    }
}
