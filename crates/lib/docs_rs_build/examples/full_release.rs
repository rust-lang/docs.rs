use anyhow::{Context as _, Result};
use docs_rs_build::{BuildEnvironment, SandboxImageSource};
use rustwide::Crate;
use std::{env, path::PathBuf};

fn main() -> Result<()> {
    let mut args = env::args_os().skip(1);
    let name = args.next().context("usage: full_release NAME VERSION")?;
    let version = args.next().context("usage: full_release NAME VERSION")?;
    let name = name.to_string_lossy();
    let version = version.to_string_lossy();

    let workspace = PathBuf::from("rustwide-workspace");
    let mut environment = BuildEnvironment::builder(workspace.as_path())
        .sandbox_image(SandboxImageSource::LocalOrRemote(
            "docsrs/build-env:latest".into(),
        ))
        .build()?;
    if environment.update_toolchain()? {
        let essential_files = environment.build_essential_files()?.into_inner();
        println!("essential files: {}", essential_files.display());
    }

    let krate = Crate::crates_io(&name, &version);
    let build = environment
        .release(&krate)
        .run(|build| build.build_docs())?;

    println!("sandbox statistics: {:#?}", build.statistics());
    let release = build.into_inner();
    for target in release.targets {
        println!("target: {}", target.target);
        println!("  documentation: {}", target.documentation.successful());
        println!("  rustdoc JSON: {}", target.rustdoc_json.successful());
        println!("  coverage: {}", target.coverage.successful());
    }

    Ok(())
}
