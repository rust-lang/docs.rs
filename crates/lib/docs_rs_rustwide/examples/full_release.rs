use anyhow::{Context as _, Result};
use docs_rs_rustwide::{BuildEnvironment, SandboxImageSource};
use rustwide::Crate;
use std::{env, path::PathBuf, time::Duration};

fn main() -> Result<()> {
    docs_rs_rustwide::logging::init(false);
    let mut args = env::args_os().skip(1);
    let name = args.next().context("usage: full_release NAME VERSION")?;
    let version = args.next().context("usage: full_release NAME VERSION")?;
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
    let build = environment
        .release(&krate)
        .run(|build| Ok(build.build_docs()))?;

    println!("sandbox statistics: {:#?}", build.statistics());
    let release_result = build.into_inner();
    for target_result in release_result.targets() {
        println!("target: {}", target_result.target());
        println!(
            "  documentation: {}",
            target_result.documentation_succeeded()
        );
        println!("  rustdoc JSON: {}", target_result.rustdoc_json().is_ok());
        println!("  coverage: {}", target_result.coverage().is_ok());
    }

    Ok(())
}
