use anyhow::{Context as _, Result};
use docs_rs_build::{BuildEnvironment, SandboxImageSource};
use rustwide::Crate;
use std::{env, path::PathBuf};

fn main() -> Result<()> {
    let mut args = env::args_os().skip(1);
    let name = args.next().context("usage: custom_build NAME VERSION")?;
    let version = args.next().context("usage: custom_build NAME VERSION")?;
    let name = name.to_string_lossy();
    let version = version.to_string_lossy();

    let workspace = PathBuf::from("rustwide-workspace");
    let mut environment = BuildEnvironment::builder(workspace.as_path())
        .sandbox_image(SandboxImageSource::LocalOrRemote(
            "docsrs/build-env:latest".into(),
        ))
        .build()?;
    if environment.update_toolchain()? {
        environment.purge_caches()?;
        let essential_files = environment.build_essential_files()?.into_inner();
        println!("essential files: {}", essential_files.display());
    }

    let krate = Crate::crates_io(&name, &version);
    let build = environment.release(&krate).run(|build| {
        let target = build.metadata_targets().default_target.to_owned();

        // Both commands run in the same prepared rustwide sandbox.
        let rustdoc_json = build.build_rustdoc_json(&target);
        let documentation = build.build_documentation(&target);

        Ok((target, rustdoc_json, documentation))
    })?;

    let (target, rustdoc_json, documentation) = build.into_inner();
    println!("target: {target}");
    println!("rustdoc JSON: {}", rustdoc_json.successful());
    println!("documentation: {}", documentation.successful());

    Ok(())
}
