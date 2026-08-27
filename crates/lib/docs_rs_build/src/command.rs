use crate::{BuildEnvironment, BuildStepError, ReleaseBuildResult, StepResult, TargetBuildResult};
use anyhow::{Context as _, Result, bail};
use docs_rs_build_limits::Limits;
use docs_rs_cargo_metadata::CargoMetadata;
use docs_rs_types::doc_coverage::{self, DocCoverage};
use docsrs_metadata::{BuildTargets, DEFAULT_TARGETS, Metadata};
use rustwide::Build;
use rustwide::{
    cmd::Command,
    logging::{self, LogStorage},
};
use std::{
    ffi::OsStr,
    fs::{self, File},
    io::{BufRead as _, BufReader},
    path::{Path, PathBuf},
};
use tracing::warn;

/// Name of rustdoc's documentation output directory.
pub const DOC_OUTPUT_DIR_NAME: &str = "doc";

const UNCONDITIONAL_RUSTDOC_ARGS: &[&str] = &[
    "--static-root-path",
    "/-/rustdoc.static/",
    "--cap-lints",
    "warn",
    "--extern-html-root-takes-precedence",
];

#[derive(Debug)]
pub enum Emit {
    HtmlStaticFiles,
    HtmlNonStaticFiles,
}

impl Emit {
    pub fn as_str(&self) -> &str {
        match self {
            Self::HtmlStaticFiles => "html-static-files",
            Self::HtmlNonStaticFiles => "html-non-static-files",
        }
    }
}

/// Options that vary between invocations of the docs.rs Cargo command.
#[derive(Clone, Debug, Default)]
pub struct CommandOptions {
    /// Extra rustdoc flags, such as the HTML or JSON output mode.
    pub rustdoc_args: Vec<String>,
}

/// A prepared release inside an active rustwide sandbox.
pub struct ReleaseBuild<'build, 'ws> {
    pub(crate) environment: &'build BuildEnvironment,
    pub(crate) build: &'build Build<'ws>,
    pub(crate) metadata: Metadata,
    pub(crate) limits: &'build Limits,
    pub(crate) resource_suffix: String,
}

impl<'build, 'ws> ReleaseBuild<'build, 'ws> {
    pub(crate) fn new(
        environment: &'build BuildEnvironment,
        build: &'build Build<'ws>,
        limits: &'build Limits,
    ) -> Result<Self> {
        let metadata = Metadata::from_crate_root(build.host_source_dir())?;
        let resource_suffix = environment.resource_suffix()?;

        Ok(Self {
            environment,
            build,
            metadata,
            limits,
            resource_suffix,
        })
    }
}

impl<'build, 'ws> ReleaseBuild<'build, 'ws> {
    /// Prepare the Cargo command used by docs.rs for one documentation target.
    ///
    /// The command runs inside this build's sandbox. Dependencies must be
    /// fetched beforehand because docs.rs invokes Cargo in offline mode.
    pub(crate) fn command<'pl>(
        &self,
        target: &str,
        options: CommandOptions,
    ) -> Result<Command<'ws, 'pl>> {
        let cargo_args = cargo_args(
            target,
            &self.metadata,
            self.environment.cargo_jobs(),
            options,
        );

        if !DEFAULT_TARGETS.contains(&target) && !uses_build_std(&cargo_args) {
            self.environment
                .configured_toolchain()
                .add_target(self.environment.workspace(), target)?;
        }

        let mut command = self
            .build
            .cargo()
            .timeout(Some(self.limits.timeout()))
            .no_output_timeout(None);

        for (key, value) in self.metadata.environment_variables() {
            command = command.env(key, value);
        }

        Ok(command.args(&cargo_args))
    }

    /// Return the host path containing documentation for a target.
    ///
    /// Cargo places proc-macro documentation in the host target directory even
    /// when a target argument is otherwise in use.
    pub fn output_dir(&self, target: &str) -> PathBuf {
        if self.metadata.proc_macro {
            self.build.host_target_dir().join(DOC_OUTPUT_DIR_NAME)
        } else {
            self.build
                .host_target_dir()
                .join(target)
                .join(DOC_OUTPUT_DIR_NAME)
        }
    }

    /// Targets selected by this release's docs.rs metadata.
    /// Fall back to the default target list, or the host-target.
    pub fn metadata_targets(&self) -> BuildTargets<'_> {
        self.metadata
            .targets(self.environment.includes_default_targets())
    }

    /// Fetch dependencies needed by `-Zbuild-std` before offline commands run.
    pub(crate) fn fetch_build_std_dependencies(&self, targets: &[&str]) -> Result<()> {
        self.build.fetch_build_std_dependencies(targets)
    }

    /// Metadata parsed from the prepared crate source.
    pub fn metadata(&self) -> &Metadata {
        &self.metadata
    }

    /// Limits applied to this release.
    pub fn limits(&self) -> &Limits {
        self.limits
    }

    /// Build coverage, rustdoc JSON, and HTML for the full docs.rs target set.
    ///
    /// All commands execute through the same rustwide build and reusable
    /// sandbox. Coverage and JSON failures are returned with their individual
    /// steps and do not prevent the primary HTML build from running.
    pub fn build_docs(self) -> Result<ReleaseBuildResult> {
        let metadata_targets = self.metadata_targets();
        let default_target = metadata_targets.default_target;
        let other_targets: Vec<_> = metadata_targets
            .other_targets
            .into_iter()
            // the default target is already extracted above.
            .take(self.limits.targets() - 1)
            .collect();

        let mut fetch_targets = Vec::with_capacity(1 + other_targets.len());
        fetch_targets.push(default_target);
        fetch_targets.extend(other_targets.iter());
        self.fetch_build_std_dependencies(&fetch_targets)?;
        let cargo_metadata = self.load_cargo_metadata()?;

        let mut default_target_result = self.build_target_inner(default_target, true)?;
        if !default_target_result.successful()
            && self.build.host_source_dir().join("Cargo.lock").exists()
        {
            self.retry_without_lockfile()?;
            default_target_result = self.build_target_inner(default_target, true)?;
        }

        let default_has_docs = default_target_result.successful()
            && cargo_metadata.root().library_name().is_some_and(|name| {
                default_target_result
                    .documentation
                    .output
                    .as_ref()
                    .is_some_and(|path| path.join(name).is_dir())
            });
        let mut target_results = vec![default_target_result];

        if default_has_docs {
            for target in other_targets {
                target_results.push(self.build_target_inner(target, false)?);
            }
        }

        Ok(ReleaseBuildResult {
            metadata: self.metadata,
            cargo_metadata,
            targets: target_results,
        })
    }

    /// Build coverage, rustdoc JSON, and HTML for one target.
    pub fn build_target(&self, target: &str) -> Result<TargetBuildResult> {
        let is_default = target == self.metadata_targets().default_target;
        self.build_target_inner(target, is_default)
    }

    fn build_target_inner(&self, target: &str, is_default: bool) -> Result<TargetBuildResult> {
        // Coverage must precede the HTML build because Cargo currently clears
        // rustdoc's target output directory between these invocations.
        let coverage_result = self.build_coverage(target);
        let rustdoc_json_result = self.build_rustdoc_json(target);
        let documentation_result = self.build_documentation(target);

        if documentation_result.successful() && self.metadata.proc_macro {
            debug_assert!(is_default, "proc macros only support their host target");
        }

        Ok(TargetBuildResult {
            target: target.into(),
            is_default,
            documentation: documentation_result,
            rustdoc_json: rustdoc_json_result,
            coverage: coverage_result,
        })
    }

    /// Collect documentation coverage for one target.
    pub fn build_coverage(&self, target: &str) -> StepResult<Option<DocCoverage>> {
        self.capture_step(|| {
            let mut coverage = DocCoverage::default();
            let rustdoc_args = vec![
                "--output-format".into(),
                "json".into(),
                "--show-coverage".into(),
            ];

            self.command(target, CommandOptions { rustdoc_args })
                .map_err(BuildStepError::Output)?
                .log_output(true)
                .run()
                .map_err(BuildStepError::Command)?;

            let output_dir = self.output_dir(target);
            if let Ok(path) = find_single_output_file(&output_dir, "json") {
                let reader = BufReader::new(File::open(path).map_err(anyhow::Error::from)?);
                for line in reader.lines() {
                    let line = line.map_err(anyhow::Error::from)?;
                    match doc_coverage::parse_line(&line) {
                        Ok(file_coverages) => coverage.extend(file_coverages),
                        Err(error) => warn!(?error, line, "failed to parse coverage line"),
                    }
                }
            }

            Ok((coverage.total_items != 0 || coverage.documented_items != 0).then_some(coverage))
        })
    }

    /// Build unstable rustdoc JSON for one target.
    pub fn build_rustdoc_json(&self, target: &str) -> StepResult<PathBuf> {
        self.capture_step(|| {
            self.command(
                target,
                CommandOptions {
                    rustdoc_args: vec!["--output-format".into(), "json".into()],
                },
            )
            .map_err(BuildStepError::Output)?
            .run()
            .map_err(BuildStepError::Command)?;

            find_single_output_file(self.output_dir(target), "json").map_err(BuildStepError::Output)
        })
    }

    /// Build HTML documentation without emitting shared static files.
    pub fn build_documentation(&self, target: &str) -> StepResult<PathBuf> {
        self.build_html(target, Emit::HtmlNonStaticFiles)
    }

    pub(crate) fn build_essential_files(&self) -> StepResult<PathBuf> {
        self.build_html(docsrs_metadata::HOST_TARGET, Emit::HtmlStaticFiles)
    }

    fn build_html(&self, target: &str, emit: Emit) -> StepResult<PathBuf> {
        self.capture_step(|| {
            let rustdoc_args = vec![
                format!("--emit={}", emit.as_str()),
                "--resource-suffix".into(),
                self.resource_suffix.clone(),
            ];

            self.command(target, CommandOptions { rustdoc_args })
                .map_err(BuildStepError::Output)?
                .arg("-Zrustdoc-scrape-examples")
                .run()
                .map_err(BuildStepError::Command)?;

            Ok(self.output_dir(target))
        })
    }

    fn capture_step<T>(&self, run: impl FnOnce() -> Result<T, BuildStepError>) -> StepResult<T> {
        let mut storage = LogStorage::new(log::LevelFilter::Info);
        storage.set_max_size(self.limits.max_log_size());
        let captured_result = logging::capture(&storage, run);

        match captured_result {
            Ok(output) => StepResult {
                output: Some(output),
                error: None,
                log: storage.to_string(),
            },
            Err(error) => StepResult {
                output: None,
                error: Some(error),
                log: storage.to_string(),
            },
        }
    }

    fn retry_without_lockfile(&self) -> Result<()> {
        let source_dir = self.build.host_source_dir();
        fs::remove_file(source_dir.join("Cargo.lock"))?;

        Command::new(
            self.environment.workspace(),
            self.environment.configured_toolchain().cargo(),
        )
        .current_directory(&source_dir)
        .arg("generate-lockfile")
        .run_capture()
        .context("generating a replacement lockfile")?;

        Command::new(
            self.environment.workspace(),
            self.environment.configured_toolchain().cargo(),
        )
        .current_directory(source_dir)
        .args(["fetch", "--locked"])
        .run_capture()
        .context("fetching dependencies for the replacement lockfile")?;

        Ok(())
    }

    fn load_cargo_metadata(&self) -> Result<CargoMetadata> {
        let output = Command::new(
            self.environment.workspace(),
            self.environment.configured_toolchain().cargo(),
        )
        .args(["metadata", "--format-version", "1"])
        .current_directory(self.build.host_source_dir())
        .log_output(false)
        .run_capture()?;

        let [metadata] = output.stdout_lines() else {
            bail!("invalid output returned by `cargo metadata`");
        };

        CargoMetadata::load_from_metadata(metadata)
    }
}

fn cargo_args(
    target: &str,
    metadata: &Metadata,
    cargo_jobs: Option<usize>,
    mut options: CommandOptions,
) -> Vec<String> {
    let mut additional_args = vec![
        "--offline".into(),
        "-Zunstable-options".into(),
        format!(
            r#"--config=doc.extern-map.registries.crates-io="https://docs.rs/{{pkg_name}}/{{version}}/{target}""#
        ),
    ];

    if let Some(jobs) = cargo_jobs {
        additional_args.push(format!("-j{jobs}"));
    }

    // Cargo puts proc-macro documentation in the host target directory and
    // does not reliably forward RUSTDOCFLAGS when --target is supplied.
    if !metadata.proc_macro {
        additional_args.push("--target".into());
        additional_args.push(target.into());
    }

    options
        .rustdoc_args
        .extend(UNCONDITIONAL_RUSTDOC_ARGS.iter().map(|arg| (*arg).into()));
    metadata.cargo_args(&additional_args, &options.rustdoc_args)
}

fn uses_build_std(args: &[String]) -> bool {
    args.iter().enumerate().any(|(index, arg)| {
        arg.starts_with("-Zbuild-std")
            || (arg == "-Z"
                && args
                    .get(index + 1)
                    .is_some_and(|next| next.starts_with("build-std")))
    })
}

fn find_single_output_file(
    directory: impl AsRef<Path>,
    extension: impl AsRef<OsStr>,
) -> Result<PathBuf> {
    let directory = directory.as_ref();
    let extension = extension.as_ref();
    let matches: Vec<_> = fs::read_dir(directory)
        .with_context(|| format!("reading rustdoc output directory {}", directory.display()))?
        .filter_map(|entry| {
            let entry = entry.ok()?;
            if !entry.file_type().ok()?.is_file() {
                return None;
            }
            let path = entry.path();
            path.extension()
                .is_some_and(|actual| actual.eq_ignore_ascii_case(extension))
                .then_some(path)
        })
        .collect();

    if matches.len() != 1 {
        bail!(
            "found {} instead of exactly one {} file in {}: {:?}",
            matches.len(),
            extension.to_string_lossy(),
            directory.display(),
            matches,
        );
    }

    Ok(matches.into_iter().next().expect("length checked above"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsStr;

    #[test]
    fn recognizes_build_std_spellings() {
        assert!(uses_build_std(&["-Zbuild-std=core".into()]));
        assert!(uses_build_std(&["-Z".into(), "build-std".into()]));
        assert!(!uses_build_std(&["-Zunstable-options".into()]));
    }

    #[test]
    fn finds_exactly_one_output_file() -> Result<()> {
        let directory = tempfile::tempdir()?;
        fs::write(directory.path().join("crate.json"), "{}")?;
        fs::write(directory.path().join("index.html"), "")?;

        assert_eq!(
            find_single_output_file(directory.path(), OsStr::new("json"))?,
            directory.path().join("crate.json")
        );
        Ok(())
    }

    #[test]
    fn rejects_ambiguous_output_files() -> Result<()> {
        let directory = tempfile::tempdir()?;
        fs::write(directory.path().join("one.json"), "{}")?;
        fs::write(directory.path().join("two.json"), "{}")?;

        let error = find_single_output_file(directory.path(), "json").unwrap_err();
        assert!(error.to_string().contains("found 2 instead of exactly one"));
        Ok(())
    }
}
