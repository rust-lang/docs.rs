mod args;
mod logging;
mod package;
mod report;

#[cfg(test)]
mod tests;

use anyhow::{Context as _, Result, bail};
use args::Args;
use clap::Parser as _;
use docs_rs_rustwide::BuildEnvironment;
use rustwide::Crate;
use std::{
    env,
    path::{Path, PathBuf},
    process::{self, ExitCode},
};
use tracing::info;

fn main() -> ExitCode {
    let args = Args::parse();
    if let Err(error) = logging::init(args.verbose) {
        eprintln!("error: {error:#}");
        return ExitCode::FAILURE;
    }

    match run(&args) {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => ExitCode::FAILURE,
        Err(error) => {
            eprintln!("error: {error:#}");
            if let Some(failure) = error.downcast_ref::<docs_rs_rustwide::StepFailure>()
                && let Some(log) = failure.log()
            {
                eprintln!("captured build log:\n{log}");
            }
            ExitCode::FAILURE
        }
    }
}

fn run(args: &Args) -> Result<bool> {
    ensure_supported_host()?;
    ensure_crate_path(&args.crate_path)?;

    let crate_path = args
        .crate_path
        .canonicalize()
        .with_context(|| format!("resolving crate path {}", args.crate_path.display()))?;
    ensure_docker_available()?;
    let packaged = package::create(&crate_path, args.package.as_deref())?;

    let workspace_path = absolute_path(&args.workspace_path())?;
    info!(crate_path = %crate_path.display(), workspace = %workspace_path.display(), "initializing docs.rs build environment");
    let mut environment = BuildEnvironment::builder(workspace_path.as_path())
        .fast_init(true)
        .toolchain(args.toolchain())
        .sandbox_image(args.sandbox_image())
        .maybe_cpu_limit(args.cpu_limit())
        .docker_runtime(args.docker_runtime())
        .rustdoc_lints(args.rustdoc_lints())
        // Build the package's configured targets without adding docs.rs's default target list.
        // This is the production configuration.
        .include_default_targets(false)
        .default_limits(args.limits())
        .build()
        .context("initializing the docs.rs build environment")?;

    if args.should_update_toolchain() {
        info!("checking the configured Rust toolchain for updates");
        environment
            .update_toolchain()
            .context("installing or updating the configured Rust toolchain")?;
    }

    info!("starting docs.rs build");
    let krate = Crate::local(packaged.source.path());
    let build = environment
        .release(&krate)
        .directory_label(packaged.directory_label)
        .run(|release| Ok(release.build_docs()))
        .context("running the docs.rs build")?;
    let duration = build.duration();
    let result = build.into_inner();
    let succeeded = report::build_succeeded(&result, args.strict);
    report::print(&result, duration, succeeded, args.strict)?;
    Ok(succeeded)
}

fn ensure_crate_path(path: &Path) -> Result<()> {
    if !path.join("Cargo.toml").is_file() {
        bail!("`{}` does not contain a Cargo.toml", path.display());
    }
    Ok(())
}

fn absolute_path(path: &Path) -> Result<PathBuf> {
    if path.is_absolute() {
        Ok(path.to_owned())
    } else {
        Ok(env::current_dir()?.join(path))
    }
}

fn ensure_docker_available() -> Result<()> {
    let output = process::Command::new("docker")
        .args(["info", "--format", "{{.ServerVersion}}"])
        .output()
        .context("running `docker info`; install Docker and ensure its daemon is reachable")?;
    if !output.status.success() {
        let details = String::from_utf8_lossy(&output.stderr);
        bail!(
            "Docker is not available; install Docker and ensure its daemon is reachable: {}",
            details.trim()
        );
    }
    Ok(())
}

#[cfg(not(target_os = "linux"))]
fn ensure_supported_host() -> Result<()> {
    bail!(
        "native docs.rs builds currently require Linux and Docker; on macOS or Windows, run this command in a Linux CI job or Linux VM"
    )
}

#[cfg(target_os = "linux")]
fn ensure_supported_host() -> Result<()> {
    Ok(())
}
