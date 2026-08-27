use anyhow::Result;
use docs_rs_build_limits::Limits;
use docsrs_metadata::{DEFAULT_TARGETS, Metadata};
use rustwide::{Build, Toolchain, Workspace, cmd::Command};
use std::path::PathBuf;

/// Name of rustdoc's documentation output directory.
pub const DOC_OUTPUT_DIR_NAME: &str = "doc";

const UNCONDITIONAL_RUSTDOC_ARGS: &[&str] = &[
    "--static-root-path",
    "/-/rustdoc.static/",
    "--cap-lints",
    "warn",
    "--extern-html-root-takes-precedence",
];

/// Options that vary between invocations of the docs.rs Cargo command.
#[derive(Clone, Debug, Default)]
pub struct CommandOptions {
    /// Extra rustdoc flags, such as the HTML or JSON output mode.
    pub rustdoc_args: Vec<String>,
    /// Cargo job count selected to match the sandbox CPU restriction.
    pub cargo_jobs: Option<usize>,
}

/// Shared environment needed to construct docs.rs build commands.
pub struct BuildContext<'a> {
    workspace: &'a Workspace,
    toolchain: &'a Toolchain,
}

impl<'a> BuildContext<'a> {
    /// Create a build context using a prepared rustwide workspace and toolchain.
    pub fn new(workspace: &'a Workspace, toolchain: &'a Toolchain) -> Self {
        Self {
            workspace,
            toolchain,
        }
    }
}

/// Docs.rs-specific operations on a rustwide build.
pub trait DocsRsBuildExt<'ws> {
    /// Prepare the Cargo command used by docs.rs for one documentation target.
    ///
    /// The command runs inside this build's sandbox. Dependencies must be
    /// fetched beforehand because docs.rs invokes Cargo in offline mode.
    fn docsrs_command<'pl>(
        &self,
        context: &BuildContext<'_>,
        target: &str,
        metadata: &Metadata,
        limits: &Limits,
        options: CommandOptions,
    ) -> Result<Command<'ws, 'pl>>;

    /// Return the host path containing documentation for a target.
    ///
    /// Cargo places proc-macro documentation in the host target directory even
    /// when a target argument is otherwise in use.
    fn docsrs_output_dir(&self, metadata: &Metadata, target: &str) -> PathBuf;
}

impl<'ws> DocsRsBuildExt<'ws> for Build<'ws> {
    fn docsrs_command<'pl>(
        &self,
        context: &BuildContext<'_>,
        target: &str,
        metadata: &Metadata,
        limits: &Limits,
        options: CommandOptions,
    ) -> Result<Command<'ws, 'pl>> {
        let cargo_args = cargo_args(target, metadata, options);

        if !DEFAULT_TARGETS.contains(&target) && !uses_build_std(&cargo_args) {
            context.toolchain.add_target(context.workspace, target)?;
        }

        let mut command = self
            .cargo()
            .timeout(Some(limits.timeout()))
            .no_output_timeout(None);

        for (key, value) in metadata.environment_variables() {
            command = command.env(key, value);
        }

        Ok(command.args(&cargo_args))
    }

    fn docsrs_output_dir(&self, metadata: &Metadata, target: &str) -> PathBuf {
        if metadata.proc_macro {
            self.host_target_dir().join(DOC_OUTPUT_DIR_NAME)
        } else {
            self.host_target_dir()
                .join(target)
                .join(DOC_OUTPUT_DIR_NAME)
        }
    }
}

fn cargo_args(target: &str, metadata: &Metadata, mut options: CommandOptions) -> Vec<String> {
    let mut additional_args = vec![
        "--offline".into(),
        "-Zunstable-options".into(),
        format!(
            r#"--config=doc.extern-map.registries.crates-io="https://docs.rs/{{pkg_name}}/{{version}}/{target}""#
        ),
    ];

    if let Some(jobs) = options.cargo_jobs {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognizes_build_std_spellings() {
        assert!(uses_build_std(&["-Zbuild-std=core".into()]));
        assert!(uses_build_std(&["-Z".into(), "build-std".into()]));
        assert!(!uses_build_std(&["-Zunstable-options".into()]));
    }
}
