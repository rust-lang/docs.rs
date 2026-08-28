use anyhow::{Context as _, Result};
use docs_rs_build::{BuildEnvironment, SandboxImageSource};
use rustwide::Crate;
use std::{env, fs, path::PathBuf};

fn main() -> Result<()> {
    let mut args = env::args_os().skip(1);
    let name = args
        .next()
        .context("usage: extract_sources NAME VERSION SOURCE_DIRECTORY")?;
    let version = args
        .next()
        .context("usage: extract_sources NAME VERSION SOURCE_DIRECTORY")?;
    let source_directory = PathBuf::from(
        args.next()
            .context("usage: extract_sources NAME VERSION SOURCE_DIRECTORY")?,
    );
    let name = name.to_string_lossy();
    let version = version.to_string_lossy();

    let environment = BuildEnvironment::builder(PathBuf::from("rustwide-workspace").as_path())
        .sandbox_image(SandboxImageSource::LocalOrRemote(
            "docsrs/build-env:latest".into(),
        ))
        .build()?;

    let krate = Crate::crates_io(&name, &version);
    let fetched = environment.release(&krate).fetch()?;

    fs::create_dir_all(&source_directory)?;
    fetched.copy_source_to(&source_directory)?;
    println!("extracted sources to {}", source_directory.display());

    // Sandbox and build preparation only start after the source copy is complete.
    let result = fetched.run(|build| build.build_docs())?;
    println!(
        "documentation succeeded: {}",
        result.into_inner().successful()
    );

    Ok(())
}
