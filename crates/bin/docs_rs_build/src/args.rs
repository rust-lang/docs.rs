use clap::{ArgAction, Parser, ValueEnum};
use docs_rs_build_limits::Limits;
use docs_rs_rustwide::{BuildCores, CpuLimit, SandboxImageSource};
use rustwide::{Toolchain, cmd::DockerRuntime};
use std::{path::PathBuf, time::Duration};

/// Run the same sandboxed documentation build used by docs.rs.
#[derive(Debug, Parser)]
#[command(version, max_term_width = 100)]
pub(crate) struct Args {
    /// Path to the crate or workspace containing the package to build.
    #[arg(default_value = ".", value_name = "CRATE_PATH")]
    pub(crate) crate_path: PathBuf,

    /// Package to build when the manifest belongs to a workspace.
    #[arg(short, long, value_name = "SPEC")]
    pub(crate) package: Option<String>,

    /// Directory used for rustwide caches and build state.
    #[arg(long, value_name = "PATH")]
    pub(crate) workspace: Option<PathBuf>,

    /// Rustup toolchain channel or version to use.
    #[arg(long, value_name = "CHANNEL", conflicts_with = "ci_toolchain")]
    toolchain: Option<String>,

    /// Rust CI artifact commit SHA to use as the toolchain.
    #[arg(long, value_name = "SHA", conflicts_with = "toolchain")]
    ci_toolchain: Option<String>,

    /// Use the alternate Rust CI artifacts.
    #[arg(long, requires = "ci_toolchain")]
    ci_alt: bool,

    /// Use the smaller crates-build-env image used by the integration tests.
    #[arg(long, conflicts_with = "image")]
    small_image: bool,

    /// Override the sandbox image name.
    #[arg(long, value_name = "IMAGE")]
    image: Option<String>,

    /// How the sandbox image is obtained.
    #[arg(long, value_enum, default_value_t)]
    image_source: ImageSource,

    /// The Docker runtime used for sandbox containers.
    #[arg(long, value_enum, default_value_t)]
    docker_runtime: DockerRuntimeArg,

    /// Do not add docs.rs's default target list when crate metadata has no targets.
    #[arg(long)]
    no_default_targets: bool,

    /// Do not check for a newer version of the selected dist toolchain.
    #[arg(long)]
    pub(crate) no_update_toolchain: bool,

    /// Treat failures of auxiliary builds and additional targets as fatal.
    #[arg(long)]
    pub(crate) strict: bool,

    /// Sandbox memory limit (for example 3GiB or 512MiB).
    #[arg(long, default_value = "3GiB", value_parser = parse_byte_size)]
    memory: usize,

    /// Maximum number of additional documentation targets.
    #[arg(long, default_value_t = 10)]
    max_targets: usize,

    /// Timeout for each Cargo command (for example 15m or 900s).
    #[arg(long, default_value = "15m", value_parser = parse_duration)]
    timeout: Duration,

    /// Allow network access inside build sandboxes.
    #[arg(long)]
    network: bool,

    /// Limit sandbox CPU time to this many CPUs, including fractional values.
    #[arg(long, value_name = "CPUS", conflicts_with = "cpu_cores", value_parser = parse_cpu_quota)]
    cpu_limit: Option<f32>,

    /// Pin sandbox execution to one core or an inclusive range (for example 2 or 2-5).
    #[arg(long, value_name = "CORE[-CORE]")]
    cpu_cores: Option<BuildCores>,

    /// Maximum amount of output retained for each build step.
    ///
    /// Output is still streamed live in full; this only limits the copy kept in the result.
    #[arg(long, default_value = "100KiB", value_parser = parse_byte_size)]
    max_captured_log_size: usize,

    /// Increase diagnostic verbosity. Repeat for trace-level output.
    #[arg(short, long, action = ArgAction::Count)]
    pub(crate) verbose: u8,
}

impl Args {
    pub(crate) fn workspace_path(&self) -> PathBuf {
        self.workspace
            .clone()
            .unwrap_or_else(|| self.crate_path.join("target/docsrs-build"))
    }

    pub(crate) fn toolchain(&self) -> Toolchain {
        match &self.ci_toolchain {
            Some(sha) => Toolchain::ci(sha, self.ci_alt),
            None => Toolchain::dist(self.toolchain.as_deref().unwrap_or("nightly")),
        }
    }

    pub(crate) fn should_update_toolchain(&self) -> bool {
        !self.no_update_toolchain && self.ci_toolchain.is_none()
    }

    pub(crate) fn sandbox_image(&self) -> SandboxImageSource {
        let name = self.image.clone().unwrap_or_else(|| {
            if self.small_image {
                docs_rs_rustwide::SANDBOX_IMAGE_LINUX_MICRO
            } else {
                docs_rs_rustwide::SANDBOX_IMAGE_LINUX
            }
            .into()
        });
        match self.image_source {
            ImageSource::LocalOrRemote => SandboxImageSource::LocalOrRemote(name),
            ImageSource::Local => SandboxImageSource::Local(name),
            ImageSource::Remote => SandboxImageSource::Remote(name),
        }
    }

    pub(crate) fn docker_runtime(&self) -> DockerRuntime {
        match self.docker_runtime {
            DockerRuntimeArg::Default => DockerRuntime::Default,
            DockerRuntimeArg::Runsc => DockerRuntime::Runsc,
        }
    }

    pub(crate) fn cpu_limit(&self) -> Option<CpuLimit> {
        self.cpu_cores
            .clone()
            .map(CpuLimit::Cores)
            .or_else(|| self.cpu_limit.map(CpuLimit::Quota))
    }

    pub(crate) fn include_default_targets(&self) -> bool {
        !self.no_default_targets
    }

    pub(crate) fn limits(&self) -> Limits {
        Limits {
            memory: self.memory,
            targets: self.max_targets,
            timeout: self.timeout,
            networking: self.network,
            max_log_size: self.max_captured_log_size,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, ValueEnum)]
enum ImageSource {
    #[default]
    LocalOrRemote,
    Local,
    Remote,
}

#[derive(Clone, Copy, Debug, Default, ValueEnum)]
enum DockerRuntimeArg {
    #[default]
    Default,
    Runsc,
}

fn parse_byte_size(value: &str) -> Result<usize, String> {
    let bytes = parse_size::parse_size(value).map_err(|error| error.to_string())?;
    usize::try_from(bytes).map_err(|_| "byte size does not fit usize".into())
}

fn parse_duration(value: &str) -> Result<Duration, String> {
    humantime::parse_duration(value).map_err(|error| error.to_string())
}

fn parse_cpu_quota(value: &str) -> Result<f32, String> {
    let quota: f32 = value
        .parse()
        .map_err(|error| format!("invalid CPU quota: {error}"))?;
    CpuLimit::Quota(quota)
        .validate()
        .map_err(|error| error.to_string())?;
    Ok(quota)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_match_docs_rs() {
        let args = Args::try_parse_from(["docs_rs_build"]).unwrap();
        assert_eq!(args.crate_path, PathBuf::from("."));
        assert_eq!(args.package, None);
        assert_eq!(
            args.workspace_path(),
            PathBuf::from("./target/docsrs-build")
        );
        assert_eq!(args.limits(), Limits::default());
        assert!(args.include_default_targets());
    }

    #[test]
    fn parses_human_readable_limits() {
        let args = Args::try_parse_from([
            "docs_rs_build",
            "--memory",
            "512MiB",
            "--timeout",
            "2h",
            "--max-captured-log-size",
            "2MB",
            "--cpu-cores",
            "2-5",
        ])
        .unwrap();
        assert_eq!(args.limits().memory, 512 * 1024 * 1024);
        assert_eq!(args.limits().timeout, Duration::from_secs(2 * 60 * 60));
        assert_eq!(args.limits().max_log_size, 2_000_000);
        assert!(
            matches!(args.cpu_limit(), Some(CpuLimit::Cores(cores)) if cores == "2-5".parse::<BuildCores>().unwrap())
        );
    }

    #[test]
    fn conflicting_image_options_are_rejected() {
        assert!(
            Args::try_parse_from(["docs_rs_build", "--small-image", "--image", "custom",]).is_err()
        );
    }

    #[test]
    fn image_source_applies_to_default_small_and_explicit_images() {
        for (image_args, expected_name) in [
            (vec![], docs_rs_rustwide::SANDBOX_IMAGE_LINUX),
            (
                vec!["--small-image"],
                docs_rs_rustwide::SANDBOX_IMAGE_LINUX_MICRO,
            ),
            (vec!["--image", "custom/image"], "custom/image"),
        ] {
            for source in ["local", "remote", "local-or-remote"] {
                let mut argv = vec!["docs_rs_build", "--image-source", source];
                argv.extend(&image_args);
                let args = Args::try_parse_from(argv).unwrap();
                let image = args.sandbox_image();
                let name = match (source, image) {
                    ("local", SandboxImageSource::Local(name))
                    | ("remote", SandboxImageSource::Remote(name))
                    | ("local-or-remote", SandboxImageSource::LocalOrRemote(name)) => name,
                    (_, image) => panic!("unexpected policy for {source}: {image:?}"),
                };
                assert_eq!(name, expected_name);
            }
        }
    }

    #[test]
    fn invalid_ranges_and_units_are_rejected() {
        assert!("5-2".parse::<BuildCores>().is_err());
        assert!(parse_byte_size("3watts").is_err());
        assert!(parse_duration("eventually").is_err());
        assert!(parse_cpu_quota("0").is_err());
        assert!(parse_cpu_quota("NaN").is_err());
    }
}
