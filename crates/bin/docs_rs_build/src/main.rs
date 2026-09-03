mod args;
mod logging;
mod package;
mod report;

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
        println!("error: {error:#}");
        return ExitCode::FAILURE;
    }

    match run(&args) {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => ExitCode::FAILURE,
        Err(error) => {
            println!("error: {error:#}");
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
    let packaged = package::create(&crate_path, args.package.as_deref())?;
    ensure_docker_available()?;

    let workspace_path = absolute_path(&args.workspace_path())?;
    info!(crate_path = %crate_path.display(), workspace = %workspace_path.display(), "initializing docs.rs build environment");
    let mut environment = BuildEnvironment::builder(workspace_path.as_path())
        .toolchain(args.toolchain())
        .sandbox_image(args.sandbox_image())
        .maybe_cpu_limit(args.cpu_limit())
        .docker_runtime(args.docker_runtime())
        .include_default_targets(args.include_default_targets())
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
    let krate = Crate::local(packaged.path());
    let build = environment
        .release(&krate)
        .run(|release| release.build_docs())
        .context("running the docs.rs build")?;
    let result = build.into_inner();
    Ok(report::print(&result, args.strict))
}

fn ensure_crate_path(path: &Path) -> Result<()> {
    if !path.join("Cargo.toml").is_file() {
        bail!("{} does not contain a Cargo.toml", path.display());
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
