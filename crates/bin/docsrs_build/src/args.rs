use anyhow::{Result, anyhow, bail};
use clap::{ArgAction, Parser, ValueEnum};
use docs_rs_build::{CpuLimit, SandboxImageSource};
use docs_rs_build_limits::Limits;
use rustwide::{Toolchain, cmd::DockerRuntime};
use std::{ops::RangeInclusive, path::PathBuf, time::Duration};

const NORMAL_IMAGE: &str = "ghcr.io/rust-lang/crates-build-env/linux";
const SMALL_IMAGE: &str = "ghcr.io/rust-lang/crates-build-env/linux-micro";

/// Run the same sandboxed documentation build used by docs.rs.
#[derive(Debug, Parser)]
#[command(version, max_term_width = 100)]
pub(crate) struct Args {
    /// Path to the crate to build.
    #[arg(default_value = ".", value_name = "CRATE_PATH")]
    pub(crate) crate_path: PathBuf,

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
    #[arg(long, value_name = "CORE[-CORE]", value_parser = parse_cpu_cores)]
    cpu_cores: Option<RangeInclusive<usize>>,

    /// Maximum amount of output retained for each build step.
    ///
    /// Output is still streamed live in full; this only limits the copy kept in the result.
    #[arg(long, default_value = "100KiB", value_parser = parse_byte_size)]
    max_captured_log_size: usize,

    /// Increase diagnostic verbosity. Repeat for trace-level output.
    #[arg(short, long, action = ArgAction::Count)]
    pub(crate) verbose: u8,

    /// When to emit ANSI colors.
    #[arg(long, value_enum, default_value_t)]
    pub(crate) color: ColorChoice,
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
                SMALL_IMAGE
            } else {
                NORMAL_IMAGE
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
        Limits::builder()
            .memory(self.memory)
            .targets(self.max_targets)
            .timeout(self.timeout)
            .networking(self.network)
            .max_log_size(self.max_captured_log_size)
            .build()
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

#[derive(Clone, Copy, Debug, Default, ValueEnum)]
pub(crate) enum ColorChoice {
    Always,
    Never,
    #[default]
    Auto,
}

fn parse_byte_size(value: &str) -> Result<usize, String> {
    let bytes = parse_size::parse_size(value).map_err(|error| error.to_string())?;
    usize::try_from(bytes).map_err(|_| "byte size does not fit usize".into())
}

fn parse_duration(value: &str) -> Result<Duration, String> {
    parse_duration_inner(value).map_err(|error| error.to_string())
}

fn parse_duration_inner(value: &str) -> Result<Duration> {
    let split_at = value
        .find(|character: char| !character.is_ascii_digit())
        .unwrap_or(value.len());
    let (number, suffix) = value.split_at(split_at);
    if number.is_empty() {
        bail!("duration must start with an integer");
    }
    let number: u64 = number.parse()?;
    let seconds = match suffix {
        "" | "s" => number,
        "m" => number
            .checked_mul(60)
            .ok_or_else(|| anyhow!("duration is too large"))?,
        "h" => number
            .checked_mul(60 * 60)
            .ok_or_else(|| anyhow!("duration is too large"))?,
        _ => bail!("unsupported duration suffix `{suffix}`; use s, m, or h"),
    };
    Ok(Duration::from_secs(seconds))
}

fn parse_cpu_cores(value: &str) -> Result<RangeInclusive<usize>, String> {
    parse_cpu_cores_inner(value).map_err(|error| error.to_string())
}

fn parse_cpu_quota(value: &str) -> Result<f32, String> {
    let quota: f32 = value
        .parse()
        .map_err(|error| format!("invalid CPU quota: {error}"))?;
    if !quota.is_finite() || quota <= 0.0 {
        return Err("CPU quota must be a positive finite number".into());
    }
    Ok(quota)
}

fn parse_cpu_cores_inner(value: &str) -> Result<RangeInclusive<usize>> {
    let (start, end) = match value.split_once('-') {
        Some((start, end)) => (start.parse()?, end.parse()?),
        None => {
            let core = value.parse()?;
            (core, core)
        }
    };
    if start > end {
        bail!("CPU core range starts after it ends");
    }
    Ok(start..=end)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_match_docs_rs() {
        let args = Args::try_parse_from(["docsrs-build"]).unwrap();
        assert_eq!(args.crate_path, PathBuf::from("."));
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
            "docsrs-build",
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
        assert!(matches!(args.cpu_limit(), Some(CpuLimit::Cores(cores)) if cores == (2..=5)));
    }

    #[test]
    fn conflicting_image_options_are_rejected() {
        assert!(
            Args::try_parse_from(["docsrs-build", "--small-image", "--image", "custom"]).is_err()
        );
    }

    #[test]
    fn invalid_ranges_and_units_are_rejected() {
        assert!(parse_cpu_cores_inner("5-2").is_err());
        assert!(parse_byte_size("3watts").is_err());
        assert!(parse_duration_inner("1day").is_err());
        assert!(parse_cpu_quota("0").is_err());
        assert!(parse_cpu_quota("NaN").is_err());
    }
}
