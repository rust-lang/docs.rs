use anyhow::{Context as _, Result};
use docs_rs_build::{BuildEnvironment, resolve_sandbox_image};
use rustwide::Crate;
use std::{env, path::PathBuf};

fn main() -> Result<()> {
    let mut args = env::args_os().skip(1);
    let name = args.next().context("usage: custom_build NAME VERSION")?;
    let version = args.next().context("usage: custom_build NAME VERSION")?;
    let name = name.to_string_lossy();
    let version = version.to_string_lossy();

    let workspace = PathBuf::from("rustwide-workspace");
    let sandbox_image = resolve_sandbox_image("docsrs/build-env:latest")?;
    let environment = BuildEnvironment::builder(workspace.as_path())
        .sandbox_image(sandbox_image)
        .build()?;

    let krate = Crate::crates_io(&name, &version);
    let build = environment.release(&krate).run(|build| {
        let target = build.selected_targets().default_target.to_owned();

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
