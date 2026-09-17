use anyhow::{Context as _, Result};
use docs_rs_rustwide::{BuildEnvironment, SandboxImageSource};
use rustwide::Crate;
use std::{env, path::PathBuf, time::Duration};

fn main() -> Result<()> {
    docs_rs_rustwide::logging::init(false);
    let mut args = env::args_os().skip(1);
    let name = args.next().context("usage: custom_build NAME VERSION")?;
    let version = args.next().context("usage: custom_build NAME VERSION")?;
    let name = name.to_string_lossy();
    let version = version.to_string_lossy();

    let workspace = PathBuf::from("rustwide-workspace");
    let mut environment = BuildEnvironment::builder(workspace.as_path())
        .sandbox_image(SandboxImageSource::local_or_remote(
            docs_rs_rustwide::SANDBOX_IMAGE_LINUX,
        ))
        // Enable maintenance explicitly; both intervals are disabled by default.
        .workspace_reinitialization_interval(Duration::from_hours(24))
        .toolchain_update_interval(Duration::from_hours(1))
        .build()?;
    let maintenance = environment.perform_maintenance()?;
    if maintenance.toolchain_updated {
        let essential_files = environment.build_essential_files()?.into_inner();
        println!("essential files: {}", essential_files.path().display());
    }

    let krate = Crate::crates_io(&name, &version);
    let build = environment.release(&krate).run(|build| {
        // we only build the default target, don't care about the rest here.
        let target = build.metadata_targets().default_target.to_owned();

        // Both commands run in the same prepared rustwide sandbox.
        // Retain all step outcomes here; callers choose which failures are fatal.
        let rustdoc_json = build.build_rustdoc_json(&target);
        let documentation = build.build_documentation(&target);

        Ok((target, rustdoc_json, documentation))
    })?;

    let (target, rustdoc_json, documentation) = build.into_inner();
    println!("target: {target}");
    println!("rustdoc JSON: {}", rustdoc_json.is_ok());
    println!("documentation: {}", documentation.is_ok());

    Ok(())
}
