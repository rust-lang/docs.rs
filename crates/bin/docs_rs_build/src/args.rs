use clap::{ArgAction, Parser};
use docs_rs_build_limits::Limits;
use docs_rs_rustwide::{BuildCores, CpuLimit, CpuQuota, ImagePullPolicy, SandboxImageSource, ToolchainExt as _};
use docs_rs_types::{ByteSize, Duration};
use rustwide::{Toolchain, cmd::DockerRuntime};
use std::{path::PathBuf, sync::LazyLock};

static DEFAULT_LIMITS: LazyLock<Limits> = LazyLock::new(Limits::default);

fn parse_dist_toolchain(value: &str) -> Result<Toolchain, String> {
    Ok(Toolchain::dist(value))
}

/// Run the same sandboxed documentation build used by docs.rs.
#[derive(Debug, Parser)]
#[command(version, max_term_width = 100)]
pub(crate) struct Args {
    /// Path to the crate or workspace containing the package to build.
    #[arg(default_value = ".", value_name = "CRATE_PATH")]
    pub(crate) crate_path: PathBuf,

    /// Select one package to build; required for virtual workspaces.
    ///
    /// If omitted, uses Cargo's default package selection, including
    /// workspace.default-members at a workspace root. Exactly one crate archive
    /// must be produced; use --package if the defaults select multiple packages.
    #[arg(short, long, value_name = "SPEC")]
    pub(crate) package: Option<String>,

    /// Directory used for rustwide caches and build state.
    ///
    /// Defaults to <CRATE_PATH>/target/docsrs-build
    #[arg(long, value_name = "PATH")]
    pub(crate) workspace: Option<PathBuf>,

    /// Rustup toolchain channel or version to use.
    #[arg(
        long,
        value_name = "CHANNEL",
        default_value_t = Toolchain::default(),
        conflicts_with = "ci_toolchain",
        value_parser = parse_dist_toolchain
    )]
    toolchain: Toolchain,

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

    /// How the sandbox image is obtained: local, remote, or local-or-remote.
    #[arg(long, default_value_t)]
    image_source: ImagePullPolicy,

    /// The Docker runtime used for sandbox containers: default or runsc.
    #[arg(long, default_value_t)]
    docker_runtime: DockerRuntime,

    /// Do not add docs.rs's default target list when crate metadata has no targets.
    #[arg(long)]
    no_default_targets: bool,

    /// Do not check for a newer version of the selected dist toolchain.
    #[arg(long)]
    pub(crate) no_update_toolchain: bool,

    /// Treat failures of auxiliary builds and additional targets as fatal.
    #[arg(long)]
    pub(crate) strict: bool,

    /// Sandbox memory limit
    #[arg(long, default_value_t = DEFAULT_LIMITS.memory)]
    memory: ByteSize,

    /// Maximum number of additional documentation targets.
    #[arg(long, default_value_t = DEFAULT_LIMITS.targets)]
    max_targets: usize,

    /// Timeout for each Cargo command (for example 15m or 900s).
    #[arg(long, default_value_t = DEFAULT_LIMITS.timeout)]
    timeout: Duration,

    /// Allow network access inside build sandboxes.
    #[arg(long, default_value_t = DEFAULT_LIMITS.networking)]
    networking: bool,

    /// Limit sandbox CPU time to this many CPUs, including fractional values.
    #[arg(long, value_name = "CPUS", conflicts_with = "cpu_cores")]
    cpu_limit: Option<CpuQuota>,

    /// Pin sandbox execution to one core or an inclusive range (for example 2 or 2-5).
    #[arg(long, value_name = "CORE[-CORE]")]
    cpu_cores: Option<BuildCores>,

    /// Maximum amount of output retained for each build step.
    ///
    /// Output is still streamed live in full; this only limits the copy kept in the result.
    #[arg(long, default_value_t = DEFAULT_LIMITS.max_log_size)]
    max_captured_log_size: ByteSize,

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
            None => self.toolchain.clone(),
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
        SandboxImageSource::Image {
            name,
            source: self.image_source,
        }
    }

    pub(crate) fn docker_runtime(&self) -> DockerRuntime {
        self.docker_runtime
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
            networking: self.networking,
            max_log_size: self.max_captured_log_size,
        }
    }
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
        assert_eq!(args.limits().memory, ByteSize::mib(512));
        assert_eq!(args.limits().timeout, Duration::from_secs(2 * 60 * 60));
        assert_eq!(args.limits().max_log_size, ByteSize::mb(2));
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
                let SandboxImageSource::Image { name, source: policy } = image else {
                    panic!("expected an explicit image");
                };
                assert_eq!(policy.to_string(), source);
                assert_eq!(name, expected_name);
            }
        }
    }

    #[test]
    fn invalid_ranges_and_units_are_rejected() {
        assert!("5-2".parse::<BuildCores>().is_err());
        assert!("3watts".parse::<ByteSize>().is_err());
        assert!(Args::try_parse_from(["docs_rs_build", "--timeout", "eventually"]).is_err());
        for value in ["0", "NaN", "inf", "invalid"] {
            assert!(Args::try_parse_from(["docs_rs_build", "--cpu-limit", value]).is_err());
        }
        let args = Args::try_parse_from(["docs_rs_build", "--cpu-limit", "0.5"]).unwrap();
        assert!(matches!(args.cpu_limit(), Some(CpuLimit::Quota(quota)) if quota.get() == 0.5));
    }
}
